//! Décodage des trames caméra vers un tampon RGB8 persistant (§7.2, D-04).

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::{Image, VisionError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Mjpeg,
    Nv12,
    Yuy2,
    /// Déjà en RGB8 (sources de test).
    Rgb,
}

/// Décodeur à tampons réutilisés : aucune allocation par trame une fois la
/// résolution connue (hors allocations internes de zune-jpeg).
#[derive(Default)]
pub struct FrameDecoder {
    rgb: Vec<u8>,
    patched: Vec<u8>,
    width: usize,
    height: usize,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Dernière image décodée.
    pub fn image(&self) -> Image<'_> {
        Image {
            data: &self.rgb[..self.width * self.height * 3],
            width: self.width,
            height: self.height,
        }
    }

    fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        let len = width * height * 3;
        if self.rgb.len() < len {
            self.rgb.resize(len, 0);
        }
    }

    /// Décode une trame. `width`/`height` sont ceux annoncés par la caméra ;
    /// pour le MJPEG, les dimensions réelles sont lues dans l'en-tête JPEG.
    pub fn decode(
        &mut self,
        format: SourceFormat,
        data: &[u8],
        width: usize,
        height: usize,
    ) -> Result<Image<'_>, VisionError> {
        match format {
            SourceFormat::Mjpeg => self.decode_mjpeg(data)?,
            SourceFormat::Nv12 => {
                check_len("NV12", data, width * height * 3 / 2)?;
                self.resize(width, height);
                nv12_to_rgb(data, width, height, &mut self.rgb);
            }
            SourceFormat::Yuy2 => {
                check_len("YUY2", data, width * height * 2)?;
                self.resize(width, height);
                yuy2_to_rgb(data, width, height, &mut self.rgb);
            }
            SourceFormat::Rgb => {
                check_len("RGB", data, width * height * 3)?;
                self.resize(width, height);
                self.rgb[..width * height * 3].copy_from_slice(&data[..width * height * 3]);
            }
        }
        Ok(self.image())
    }

    fn decode_mjpeg(&mut self, data: &[u8]) -> Result<(), VisionError> {
        // Beaucoup de webcams omettent les tables de Huffman (DHT) : on insère
        // alors les tables standard avant le premier scan.
        if needs_huffman_tables(data) {
            let mut patched = std::mem::take(&mut self.patched);
            insert_huffman_tables(data, &mut patched);
            let result = self.decode_jpeg(&patched);
            self.patched = patched;
            result
        } else {
            self.decode_jpeg(data)
        }
    }

    fn decode_jpeg(&mut self, data: &[u8]) -> Result<(), VisionError> {
        let err = |message: String| VisionError::Decode {
            format: "MJPEG",
            message,
        };
        let options = DecoderOptions::new_fast().jpeg_set_out_colorspace(ColorSpace::RGB);
        let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options);
        decoder.decode_headers().map_err(|e| err(e.to_string()))?;
        let info = decoder
            .info()
            .ok_or_else(|| err("en-tête JPEG incomplet".into()))?;
        let (width, height) = (usize::from(info.width), usize::from(info.height));
        if width == 0 || height == 0 {
            return Err(err("image vide".into()));
        }
        self.resize(width, height);
        decoder
            .decode_into(&mut self.rgb[..width * height * 3])
            .map_err(|e| err(e.to_string()))
    }
}

fn check_len(format: &'static str, data: &[u8], expected: usize) -> Result<(), VisionError> {
    if data.len() < expected {
        return Err(VisionError::Decode {
            format,
            message: format!("trame de {} octets, {expected} attendus", data.len()),
        });
    }
    Ok(())
}

/// Position du marqueur SOS et présence d'un DHT avant lui.
fn scan_markers(data: &[u8]) -> Option<(usize, bool)> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    let mut has_dht = false;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        match marker {
            0xFF => {
                i += 1;
                continue;
            }
            0xDA => return Some((i, has_dht)),
            0xC4 => has_dht = true,
            0xD9 => return None,
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        if len < 2 {
            return None;
        }
        i += 2 + len;
    }
    None
}

fn needs_huffman_tables(data: &[u8]) -> bool {
    matches!(scan_markers(data), Some((_, false)))
}

fn insert_huffman_tables(data: &[u8], out: &mut Vec<u8>) {
    let Some((sos, _)) = scan_markers(data) else {
        out.clear();
        out.extend_from_slice(data);
        return;
    };
    out.clear();
    out.extend_from_slice(&data[..sos]);
    for (class_id, bits, values) in [
        (0x00u8, &DC_LUMA_BITS, &DC_VALUES[..]),
        (0x10, &AC_LUMA_BITS, &AC_LUMA_VALUES[..]),
        (0x01, &DC_CHROMA_BITS, &DC_VALUES[..]),
        (0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALUES[..]),
    ] {
        let len = (2 + 1 + 16 + values.len()) as u16;
        out.extend_from_slice(&[0xFF, 0xC4]);
        out.extend_from_slice(&len.to_be_bytes());
        out.push(class_id);
        out.extend_from_slice(bits);
        out.extend_from_slice(values);
    }
    out.extend_from_slice(&data[sos..]);
}

// Tables de Huffman standard (ITU-T T.81, annexe K.3), celles que le MJPEG suppose.
const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7D];
const AC_LUMA_VALUES: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7,
    0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5,
    0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2,
    0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8,
    0xF9, 0xFA,
];
const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALUES: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xA1, 0xB1, 0xC1, 0x09, 0x23, 0x33, 0x52, 0xF0,
    0x15, 0x62, 0x72, 0xD1, 0x0A, 0x16, 0x24, 0x34, 0xE1, 0x25, 0xF1, 0x17, 0x18, 0x19, 0x1A, 0x26,
    0x27, 0x28, 0x29, 0x2A, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5,
    0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3,
    0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA,
    0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8,
    0xF9, 0xFA,
];

fn yuv_to_rgb(y: u8, u: u8, v: u8, out: &mut [u8]) {
    // BT.601, plage limitée, en virgule fixe.
    let c = (i32::from(y) - 16) * 298;
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    out[0] = ((c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
    out[1] = ((c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
    out[2] = ((c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
}

fn nv12_to_rgb(data: &[u8], width: usize, height: usize, rgb: &mut [u8]) {
    let (luma, chroma) = data.split_at(width * height);
    for y in 0..height {
        let uv_row = &chroma[(y / 2) * width..];
        for x in 0..width {
            let uv = (x / 2) * 2;
            let o = (y * width + x) * 3;
            yuv_to_rgb(
                luma[y * width + x],
                uv_row[uv],
                uv_row[uv + 1],
                &mut rgb[o..o + 3],
            );
        }
    }
}

fn yuy2_to_rgb(data: &[u8], width: usize, height: usize, rgb: &mut [u8]) {
    for y in 0..height {
        let row = &data[y * width * 2..];
        for x in 0..width {
            let pair = (x / 2) * 4;
            let o = (y * width + x) * 3;
            yuv_to_rgb(row[x * 2], row[pair + 1], row[pair + 3], &mut rgb[o..o + 3]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_tables_are_consistent() {
        for (bits, values) in [
            (&DC_LUMA_BITS, DC_VALUES.len()),
            (&DC_CHROMA_BITS, DC_VALUES.len()),
            (&AC_LUMA_BITS, AC_LUMA_VALUES.len()),
            (&AC_CHROMA_BITS, AC_CHROMA_VALUES.len()),
        ] {
            assert_eq!(bits.iter().map(|b| usize::from(*b)).sum::<usize>(), values);
        }
        // Chaque symbole AC n'apparaît qu'une fois.
        for table in [&AC_LUMA_VALUES, &AC_CHROMA_VALUES] {
            let mut seen = [false; 256];
            for v in table.iter() {
                assert!(!seen[usize::from(*v)], "symbole {v:#x} dupliqué");
                seen[usize::from(*v)] = true;
            }
        }
    }

    fn fake_jpeg(with_dht: bool) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        j.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x04, 0x00, 0x01]); // DQT factice
        if with_dht {
            j.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x03, 0x00]);
        }
        j.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x12, 0x34, 0xFF, 0xD9]);
        j
    }

    #[test]
    fn detects_missing_dht() {
        assert!(needs_huffman_tables(&fake_jpeg(false)));
        assert!(!needs_huffman_tables(&fake_jpeg(true)));
        assert!(!needs_huffman_tables(b"pas un jpeg"));
    }

    #[test]
    fn inserts_tables_before_sos() {
        let src = fake_jpeg(false);
        let mut out = Vec::new();
        insert_huffman_tables(&src, &mut out);
        assert_eq!(
            out.len(),
            src.len() + 2 * (4 + 17 + 12) + 2 * (4 + 17 + 162)
        );
        assert!(!needs_huffman_tables(&out));
        let (sos, has_dht) = scan_markers(&out).unwrap();
        assert!(has_dht);
        assert_eq!(&out[sos..], &src[8..]);
    }

    #[test]
    fn yuv_conversion_reference_points() {
        let mut px = [0u8; 3];
        yuv_to_rgb(16, 128, 128, &mut px);
        assert_eq!(px, [0, 0, 0]);
        yuv_to_rgb(235, 128, 128, &mut px);
        assert_eq!(px, [255, 255, 255]);
        yuv_to_rgb(81, 90, 240, &mut px);
        assert!(px[0] > 250 && px[1] < 5 && px[2] < 5, "{px:?}");
    }

    #[test]
    fn nv12_and_yuy2_gray_frames() {
        let mut d = FrameDecoder::new();
        let nv12 = [vec![128u8; 4 * 2], vec![128u8; 4]].concat();
        let img = d.decode(SourceFormat::Nv12, &nv12, 4, 2).unwrap();
        assert_eq!((img.width, img.height), (4, 2));
        assert!(img.data.iter().all(|v| (*v as i32 - 130).abs() <= 1));
        let yuy2 = vec![128u8; 4 * 2 * 2];
        let img = d.decode(SourceFormat::Yuy2, &yuy2, 4, 2).unwrap();
        assert!(img.data.iter().all(|v| (*v as i32 - 130).abs() <= 1));
        assert!(d.decode(SourceFormat::Nv12, &nv12[..5], 4, 2).is_err());
    }
}
