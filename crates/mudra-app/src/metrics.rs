//! Histogrammes de latence par étage (D-18, EF-71) et mesures du processus.

use hdrhistogram::Histogram;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Decode,
    Palm,
    Landmarks,
    Gestures,
    Total,
}

impl Stage {
    pub const ALL: [Stage; 5] = [
        Stage::Decode,
        Stage::Palm,
        Stage::Landmarks,
        Stage::Gestures,
        Stage::Total,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Stage::Decode => "Décodage MJPEG",
            Stage::Palm => "Détection de paume",
            Stage::Landmarks => "Points-clés",
            Stage::Gestures => "Classifieurs + moteur",
            Stage::Total => "Total (trame à décision)",
        }
    }

    /// Budget p95 indicatif du cahier des charges (§6.1), en millisecondes.
    pub fn budget_ms(self) -> f32 {
        match self {
            Stage::Decode => 3.0,
            Stage::Palm => 4.0,
            Stage::Landmarks => 3.0,
            Stage::Gestures => 0.7,
            Stage::Total => 8.0,
        }
    }
}

/// Quantiles d'un étage, en millisecondes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quantiles {
    pub stage: Stage,
    pub count: u64,
    pub p50: f32,
    pub p95: f32,
    pub p99: f32,
}

fn new_histogram() -> Histogram<u32> {
    // 1 µs à 10 s, 2 chiffres significatifs : taille fixe, aucune allocation à l'enregistrement.
    Histogram::new_with_bounds(1, 10_000_000, 2).expect("bornes d'histogramme valides")
}

fn quantiles(stage: Stage, h: &Histogram<u32>) -> Quantiles {
    let ms = |q: f64| h.value_at_quantile(q) as f32 / 1000.0;
    Quantiles {
        stage,
        count: h.len(),
        p50: ms(0.50),
        p95: ms(0.95),
        p99: ms(0.99),
    }
}

/// Deux jeux d'histogrammes : une fenêtre récente (remise à zéro périodiquement,
/// pour l'aperçu) et un cumul (pour `mudra bench`).
pub struct Metrics {
    window: [Histogram<u32>; 5],
    total: [Histogram<u32>; 5],
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            window: std::array::from_fn(|_| new_histogram()),
            total: std::array::from_fn(|_| new_histogram()),
        }
    }
}

impl Metrics {
    pub fn record(&mut self, stage: Stage, micros: u64) {
        let i = stage as usize;
        let v = micros.max(1);
        self.window[i].saturating_record(v);
        self.total[i].saturating_record(v);
    }

    pub fn recent(&self) -> Vec<Quantiles> {
        Stage::ALL
            .iter()
            .map(|s| quantiles(*s, &self.window[*s as usize]))
            .collect()
    }

    pub fn cumulative(&self) -> Vec<Quantiles> {
        Stage::ALL
            .iter()
            .map(|s| quantiles(*s, &self.total[*s as usize]))
            .collect()
    }

    pub fn reset_window(&mut self) {
        self.window.iter_mut().for_each(Histogram::reset);
    }

    pub fn reset_all(&mut self) {
        self.reset_window();
        self.total.iter_mut().for_each(Histogram::reset);
    }
}

fn filetime_micros(t: FILETIME) -> u64 {
    ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) / 10
}

/// Temps CPU (noyau + utilisateur) consommé par le processus, en microsecondes.
pub fn process_cpu_micros() -> u64 {
    let (mut creation, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: pseudo-handle du processus courant, structures de sortie valides.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if ok.is_ok() {
        filetime_micros(kernel) + filetime_micros(user)
    } else {
        0
    }
}

/// Mémoire de travail du processus (working set), en octets.
pub fn process_memory_bytes() -> u64 {
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    let size = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: structure de sortie de la bonne taille.
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) };
    if ok.is_ok() {
        counters.WorkingSetSize as u64
    } else {
        0
    }
}

/// Mesure la part de CPU du processus entre deux appels, en pourcentage de la
/// machine entière (tous cœurs confondus), comme dans ENF-04.
pub struct CpuMeter {
    last_cpu: u64,
    last_wall: std::time::Instant,
    cores: f32,
}

impl Default for CpuMeter {
    fn default() -> Self {
        Self {
            last_cpu: process_cpu_micros(),
            last_wall: std::time::Instant::now(),
            cores: std::thread::available_parallelism().map_or(1.0, |n| n.get() as f32),
        }
    }
}

impl CpuMeter {
    pub fn sample(&mut self) -> f32 {
        let cpu = process_cpu_micros();
        let wall = self.last_wall.elapsed().as_micros().max(1) as f32;
        let percent = (cpu.saturating_sub(self.last_cpu)) as f32 / wall / self.cores * 100.0;
        self.last_cpu = cpu;
        self.last_wall = std::time::Instant::now();
        percent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_in_milliseconds() {
        let mut m = Metrics::default();
        for us in 1..=1000u64 {
            m.record(Stage::Decode, us * 10);
        }
        let q = m.recent()[Stage::Decode as usize];
        assert_eq!(q.count, 1000);
        assert!((q.p50 - 5.0).abs() < 0.1, "{}", q.p50);
        assert!((q.p95 - 9.5).abs() < 0.2, "{}", q.p95);
        m.reset_window();
        assert_eq!(m.recent()[Stage::Decode as usize].count, 0);
        assert_eq!(m.cumulative()[Stage::Decode as usize].count, 1000);
    }

    #[test]
    fn process_counters_are_plausible() {
        assert!(process_memory_bytes() > 1_000_000);
        let mut meter = CpuMeter::default();
        // Le compteur de temps CPU de Windows avance par pas d'environ 15 ms.
        let started = std::time::Instant::now();
        let mut x = 0u64;
        while started.elapsed() < std::time::Duration::from_millis(80) {
            x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
        }
        assert!(process_cpu_micros() > 0);
        assert!(meter.sample() >= 0.0);
    }
}
