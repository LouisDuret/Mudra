//! Sous-commandes en ligne de commande (EF-73) : `devices`, `record`, `replay`,
//! `bench`, `check-config` et `image` (diagnostic).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use mudra_config::{Config, KnownGestures, Level};
use mudra_gestures::stage::{GestureStage, HandInput};
use mudra_gestures::{EngineEvent, Handedness};
use mudra_vision::{FrameDecoder, HandTracker, Provider, SourceFormat, VisionSessions};

use crate::metrics::{CpuMeter, Quantiles, process_memory_bytes};
use crate::pipeline::{
    self, Command, Options, PipelineEvent, RecordEvent, RecordKind, RecordRequest, Source,
    load_classifiers, tracker_config,
};

/// `mudra devices` : caméras et formats natifs (EF-01).
pub fn devices() -> Result<()> {
    let devices = mudra_capture::list_devices()?;
    if devices.is_empty() {
        println!("Aucune caméra détectée.");
        return Ok(());
    }
    for device in devices {
        println!("[{}] {}", device.index, device.name);
        if device.formats.is_empty() {
            println!("      (formats indisponibles : caméra occupée ?)");
        }
        // Une ligne par sous-type et résolution, avec les fréquences disponibles.
        let mut groups: Vec<(String, u32, u32, bool, Vec<f32>)> = Vec::new();
        for f in &device.formats {
            match groups
                .iter_mut()
                .find(|g| g.0 == f.subtype && g.1 == f.width && g.2 == f.height)
            {
                Some(group) => group.4.push(f.fps),
                None => groups.push((
                    f.subtype.clone(),
                    f.width,
                    f.height,
                    f.pixel_format.is_some(),
                    vec![f.fps],
                )),
            }
        }
        groups.sort_by(|a, b| (&a.0, b.1 * b.2).cmp(&(&b.0, a.1 * a.2)));
        for (subtype, width, height, usable, mut rates) in groups {
            rates.sort_unstable_by(|a, b| b.total_cmp(a));
            rates.dedup();
            let rates: Vec<String> = rates.iter().map(|r| format!("{r}")).collect();
            let note = if usable { "" } else { "  (non pris en charge)" };
            println!(
                "      {subtype:<5} {width:>4}×{height:<4}  {} fps{note}",
                rates.join(", ")
            );
        }
    }
    Ok(())
}

/// Gestes connus des classifieurs réellement chargés (D-10).
pub fn known_gestures(config: &Config) -> (KnownGestures, Vec<String>) {
    let (static_classifier, dynamic_classifier, notices) = load_classifiers(config);
    (
        KnownGestures {
            static_: static_classifier.labels().to_vec(),
            dynamic: dynamic_classifier.labels().to_vec(),
            two_hands: static_classifier.two_hand_labels().to_vec(),
        },
        notices,
    )
}

/// `mudra check-config` : même validation qu'au chargement (§7.9).
pub fn check_config(path: &Path) -> Result<()> {
    println!("Configuration : {}", path.display());
    let config = mudra_config::load(path)?;
    let (known, notices) = known_gestures(&config);
    let mut errors = 0;
    for notice in notices {
        println!("  info · {notice}");
    }
    for issue in mudra_config::validate(&config, &known) {
        println!("  {issue}");
        errors += usize::from(issue.level == Level::Error);
    }
    for (name, raw) in [
        ("models.palm", &config.models.palm),
        ("models.landmarks", &config.models.landmarks),
    ] {
        let resolved = mudra_config::resolve_model_path(raw);
        if !resolved.is_file() {
            println!(
                "  erreur · {name} : {} introuvable (lance scripts/fetch-models.ps1)",
                resolved.display()
            );
            errors += 1;
        }
    }
    println!(
        "  {} bindings, {} contrôles continus · poses : {} · gestes dynamiques : {}",
        config.bindings.len(),
        config.continuous.len(),
        known.static_.join(", "),
        known.dynamic.join(", ")
    );
    if errors > 0 {
        bail!("{errors} erreur(s) : la configuration serait refusée");
    }
    println!("Configuration valide.");
    Ok(())
}

/// `mudra image <fichier.jpg>` : passe une photo dans la vision et le
/// classifieur statique, et affiche ce qui est vu. Outil de diagnostic.
pub fn image(
    config: &Config,
    path: &Path,
    frames: usize,
    mirror: bool,
    verbose: bool,
) -> Result<()> {
    let palm = mudra_config::resolve_model_path(&config.models.palm);
    let landmarks = mudra_config::resolve_model_path(&config.models.landmarks);
    let sessions = VisionSessions::load(&palm, &landmarks, pipeline::provider_for(config))?;
    println!("Fournisseur d'exécution : {}", sessions.provider.as_str());
    let mut tracker_cfg = tracker_config(config);
    tracker_cfg.mirror = mirror;
    tracker_cfg.max_hands = 2;
    let mut tracker = HandTracker::new(sessions, tracker_cfg);
    let (static_classifier, dynamic_classifier, _) = load_classifiers(config);
    println!("Classifieur de poses : {}", static_classifier.description());
    let mut stage = GestureStage::new(config, static_classifier, dynamic_classifier);

    let bytes = std::fs::read(path).with_context(|| format!("lecture de {}", path.display()))?;
    let mut decoder = FrameDecoder::new();
    let image = decoder.decode(SourceFormat::Mjpeg, &bytes, 0, 0)?;
    println!(
        "Image : {}×{} · miroir : {}",
        image.width,
        image.height,
        if mirror { "oui" } else { "non" }
    );

    let mut hands = Vec::new();
    let mut events = Vec::new();
    for frame in 0..frames.max(1) {
        tracker.process(&image, &mut hands)?;
        let inputs: Vec<HandInput> = hands
            .iter()
            .map(|h| HandInput {
                track: h.track_id,
                handedness: if h.right {
                    Handedness::Right
                } else {
                    Handedness::Left
                },
                image: h.image,
                world: h.world,
            })
            .collect();
        let now = 1_000_000 + frame as u64 * 16_667;
        stage.process(
            now,
            image.width as f32,
            image.height as f32,
            &inputs,
            &mut events,
        )?;
    }
    if hands.is_empty() {
        println!("Aucune main détectée.");
    }
    for view in stage.hands() {
        let Some(hand) = hands.iter().find(|h| h.track_id == view.track) else {
            continue;
        };
        println!(
            "Main {} · piste {} · présence {:.2} · latéralité {:.2} · boîte [{:.0}, {:.0}, {:.0}, {:.0}] · pincement {:.2}",
            if hand.right { "droite" } else { "gauche" },
            hand.track_id,
            hand.presence,
            hand.handedness_score,
            hand.bbox[0],
            hand.bbox[1],
            hand.bbox[2],
            hand.bbox[3],
            view.pinch,
        );
        let mut order: Vec<usize> = (0..view.static_probs.len()).collect();
        order.sort_unstable_by(|a, b| view.static_probs[*b].total_cmp(&view.static_probs[*a]));
        let top: Vec<String> = order
            .iter()
            .take(3)
            .map(|i| {
                format!(
                    "{} {:.0} %",
                    stage.static_labels()[*i],
                    view.static_probs[*i] * 100.0
                )
            })
            .collect();
        println!("    poses : {}", top.join(" · "));
        if verbose {
            println!(
                "    {}",
                mudra_gestures::builtin::FingerState::describe(&view.world)
            );
            let t = (view.image[4], view.image[2]);
            println!(
                "    pouce (image) : MCP ({:.0}, {:.0}) → bout ({:.0}, {:.0})",
                t.1[0], t.1[1], t.0[0], t.0[1]
            );
        }
    }
    Ok(())
}

fn print_quantiles(quantiles: &[Quantiles]) {
    println!(
        "  {:<28} {:>8} {:>9} {:>9} {:>9}   budget p95",
        "étage", "trames", "p50 (ms)", "p95 (ms)", "p99 (ms)"
    );
    for q in quantiles {
        let verdict = if q.count == 0 {
            ""
        } else if q.p95 <= q.stage.budget_ms() {
            "ok"
        } else {
            "DÉPASSÉ"
        };
        println!(
            "  {:<28} {:>8} {:>9.2} {:>9.2} {:>9.2}   ≤ {:.1} ms {verdict}",
            q.stage.label(),
            q.count,
            q.p50,
            q.p95,
            q.p99,
            q.stage.budget_ms()
        );
    }
}

/// `mudra bench` : mesure de bout en bout (B-03).
pub fn bench(config: Arc<Config>, source: Source, duration: Duration) -> Result<()> {
    let started = Instant::now();
    let pipeline = pipeline::start(Options {
        config: config.clone(),
        source,
        actions: None,
        notify: None,
        start_paused: false,
    })?;
    // Laisse la caméra démarrer et les modèles se préchauffer avant de mesurer.
    let warmup = Duration::from_secs(3).min(duration / 4);
    println!(
        "Préchauffage {:.0} s, puis mesure pendant {:.0} s…",
        warmup.as_secs_f32(),
        duration.as_secs_f32()
    );
    let mut first_frame = None;
    while started.elapsed() < warmup + Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
        let status = pipeline.status();
        if first_frame.is_none() && status.image_size.0 > 0 {
            first_frame = Some(started.elapsed());
        }
        if first_frame.is_some() && started.elapsed() >= warmup {
            break;
        }
    }
    let Some(first_frame) = first_frame else {
        let status = pipeline.status();
        bail!(
            "aucune trame reçue de la caméra ({:?})",
            status.capture.state
        );
    };
    pipeline.send(Command::ResetMetrics);
    let mut cpu = CpuMeter::default();
    let mut with_hand = 0u32;
    let mut samples = 0u32;
    let mut fps_sum = 0.0;
    let mut capture_fps_sum = 0.0;
    let measure_start = Instant::now();
    while measure_start.elapsed() < duration {
        std::thread::sleep(Duration::from_millis(500));
        let status = pipeline.status();
        samples += 1;
        with_hand += u32::from(!status.hands.is_empty());
        fps_sum += status.processed_fps;
        capture_fps_sum += status.capture.measured_fps;
        for event in pipeline.events.try_iter() {
            if let PipelineEvent::Notice { message, .. } = event {
                println!("  · {message}");
            }
        }
    }
    let cpu_percent = cpu.sample();
    let status = pipeline.status();
    let cumulative = pipeline
        .shared
        .cumulative
        .lock()
        .map(|c| c.clone())
        .unwrap_or_default();
    let samples_f = samples.max(1) as f32;

    println!();
    println!("Mudra bench — {:.0} s", duration.as_secs_f32());
    if let Some(f) = &status.capture.format {
        println!(
            "  Caméra          : {} · {} {}×{} à {:.0} fps",
            status.capture.device, f.subtype, status.image_size.0, status.image_size.1, f.fps
        );
    }
    println!("  Fournisseur     : {}", status.provider);
    println!(
        "  Classifieurs    : poses {} · dynamiques {}",
        status.static_classifier, status.dynamic_classifier
    );
    println!(
        "  Première trame  : {:.2} s après le lancement",
        first_frame.as_secs_f32()
    );
    println!(
        "  Capture         : {:.1} fps en moyenne",
        capture_fps_sum / samples_f
    );
    println!(
        "  Traitement      : {:.1} fps en moyenne",
        fps_sum / samples_f
    );
    println!(
        "  Main visible    : {:.0} % du temps",
        100.0 * with_hand as f32 / samples_f
    );
    println!(
        "  Trames abandonnées : {} ({:.2} %)",
        status.dropped, status.dropped_percent
    );
    println!("  CPU du processus   : {:.1} % de la machine", cpu_percent);
    println!(
        "  Mémoire de travail : {:.0} Mo",
        process_memory_bytes() as f32 / (1024.0 * 1024.0)
    );
    println!();
    print_quantiles(&cumulative);
    Ok(())
}

/// `mudra bench --providers` : compare les fournisseurs d'exécution (B-01).
pub fn bench_providers(config: &Config) -> Result<()> {
    let palm_path = mudra_config::resolve_model_path(&config.models.palm);
    let landmarks_path = mudra_config::resolve_model_path(&config.models.landmarks);
    // Image de synthèse : la latence d'inférence ne dépend pas du contenu.
    let (w, h) = (1280usize, 720usize);
    let data: Vec<u8> = (0..w * h * 3)
        .map(|i| ((i * 31 + i / 7) % 251) as u8)
        .collect();
    let image = mudra_vision::Image::new(&data, w, h);
    let roi = mudra_vision::Roi {
        cx: 640.0,
        cy: 360.0,
        size: 300.0,
        rotation: 0.2,
    };
    println!("B-01 — 100 inférences de préchauffage puis 1000 mesurées, image 1280×720");
    println!(
        "  {:<10} {:<22} {:>9} {:>9} {:>9} {:>12}",
        "EP", "modèle", "p50 (ms)", "p95 (ms)", "p99 (ms)", "chargement"
    );
    for requested in [Provider::Cpu, Provider::DirectMl] {
        let load_start = Instant::now();
        let mut sessions = match VisionSessions::load(&palm_path, &landmarks_path, requested) {
            Ok(s) if s.provider == requested => s,
            Ok(_) => {
                println!(
                    "  {:<10} indisponible sur cette machine (repli CPU)",
                    requested.as_str()
                );
                continue;
            }
            Err(e) => {
                println!("  {:<10} erreur : {e}", requested.as_str());
                continue;
            }
        };
        let load = load_start.elapsed();
        let mut detections = Vec::new();
        let run = |name: &str, f: &mut dyn FnMut() -> Result<()>| -> Result<()> {
            for _ in 0..100 {
                f()?;
            }
            let mut times: Vec<f32> = Vec::with_capacity(1000);
            for _ in 0..1000 {
                let t = Instant::now();
                f()?;
                times.push(t.elapsed().as_secs_f32() * 1000.0);
            }
            times.sort_unstable_by(f32::total_cmp);
            println!(
                "  {:<10} {:<22} {:>9.2} {:>9.2} {:>9.2} {:>9.0} ms",
                requested.as_str(),
                name,
                times[499],
                times[949],
                times[989],
                load.as_secs_f32() * 1000.0
            );
            Ok(())
        };
        run("paume 192×192", &mut || {
            Ok(sessions.palm.detect(&image, true, 0.5, &mut detections)?)
        })?;
        run("points-clés 224×224", &mut || {
            sessions
                .landmarks
                .run(&image, &roi, true)
                .map(drop)
                .map_err(Into::into)
        })?;
    }
    println!("  (les temps incluent le recadrage affine et la lecture des sorties)");
    Ok(())
}

/// `mudra bench --decode` : débit du décodage MJPEG sur de vraies trames de la
/// caméra, gardées en mémoire uniquement (B-02, ENF-21).
pub fn bench_decode(config: &Config, source: &Source) -> Result<()> {
    use mudra_capture::{FrameSource, MfSource, StillSource};
    let mut camera: Box<dyn FrameSource> = match source {
        Source::Camera => Box::new(MfSource::open(&pipeline::capture_config(config))?),
        Source::Still(path) => Box::new(StillSource::open(path, 60)?),
    };
    let (name, format) = camera.description();
    let mut frame = mudra_capture::Frame::default();
    // Les premières trames servent au réglage de l'exposition.
    for _ in 0..30 {
        camera.next_frame(&mut frame)?;
    }
    let mut frames = Vec::new();
    for _ in 0..40 {
        camera.next_frame(&mut frame)?;
        frames.push(frame.data.clone());
    }
    drop(camera);
    if frame.format != mudra_capture::PixelFormat::Mjpeg {
        bail!("le format ouvert n'est pas du MJPEG ({})", format.subtype);
    }
    let mut decoder = FrameDecoder::new();
    let mut times = Vec::with_capacity(frames.len() * 10);
    let (mut width, mut height) = (0, 0);
    for round in 0..11 {
        for data in &frames {
            let t = Instant::now();
            let image = decoder.decode(SourceFormat::Mjpeg, data, 0, 0)?;
            (width, height) = (image.width, image.height);
            // Le premier tour préchauffe les tampons.
            if round > 0 {
                times.push(t.elapsed().as_secs_f32() * 1000.0);
            }
        }
    }
    times.sort_unstable_by(f32::total_cmp);
    let q = |p: f32| times[((times.len() - 1) as f32 * p) as usize];
    let bytes = frames.iter().map(Vec::len).sum::<usize>() / frames.len();
    println!(
        "B-02 — décodage MJPEG (zune-jpeg), {} décodages en boucle serrée",
        times.len()
    );
    println!(
        "  Source  : {name} · {width}×{height} · trames de {:.0} Ko en moyenne",
        bytes as f32 / 1024.0
    );
    println!(
        "  p50 {:.2} ms · p95 {:.2} ms · p99 {:.2} ms · {:.0} Mpixels/s",
        q(0.5),
        q(0.95),
        q(0.99),
        (width * height) as f32 / 1e6 / (q(0.5) / 1000.0)
    );
    Ok(())
}

/// `mudra replay <session>` : rejoue une session enregistrée dans le moteur,
/// sans caméra, et affiche les événements produits (EF-64).
pub fn replay(config: &Config, session: &Path, verbose: bool) -> Result<()> {
    let (header, frames) = mudra_dataset::read_session(session)?;
    let (static_classifier, dynamic_classifier, _) = load_classifiers(config);
    println!(
        "Session {} · label « {} » · {} trames · {}×{} à {:.0} fps · classifieurs : {} / {}",
        session.display(),
        header.label,
        frames.len(),
        header.width,
        header.height,
        header.fps,
        static_classifier.description(),
        dynamic_classifier.description()
    );
    let mut stage = GestureStage::new(config, static_classifier, dynamic_classifier);
    let (width, height) = (header.width.max(1) as f32, header.height.max(1) as f32);
    let Some(start) = frames.first().map(|f| f.t) else {
        println!("Session vide.");
        return Ok(());
    };
    let mut events = Vec::new();
    let mut fired = 0usize;
    let mut transitions = 0usize;
    let mut i = 0;
    let mut previous_t = start;
    while i < frames.len() {
        let t = frames[i].t;
        // Un trou dans la session (pause entre deux clips) : le moteur voit le temps passer sans main.
        if t.saturating_sub(previous_t) > 200_000 {
            let mut gap = previous_t + 50_000;
            while gap < t {
                stage.process(gap, width, height, &[], &mut events)?;
                gap += 50_000;
            }
        }
        previous_t = t;
        let mut inputs = Vec::new();
        while i < frames.len() && frames[i].t == t {
            let f = &frames[i];
            let mut image = f.image;
            for p in image.iter_mut() {
                *p = [p[0] * width, p[1] * height, p[2] * width];
            }
            inputs.push(HandInput {
                track: f.track,
                handedness: match f.hand {
                    mudra_dataset::Hand::Left => Handedness::Left,
                    mudra_dataset::Hand::Right => Handedness::Right,
                },
                image,
                world: f.world,
            });
            i += 1;
        }
        stage.process(t, width, height, &inputs, &mut events)?;
        for event in events.drain(..) {
            let at = (t - start) as f32 * 1e-6;
            match event {
                EngineEvent::StateChanged { from, to, reason } => {
                    transitions += 1;
                    println!(
                        "{at:>8.3} s  état     {} → {} ({reason:?})",
                        from.label_fr(),
                        to.label_fr()
                    );
                }
                EngineEvent::GestureFired {
                    binding,
                    gesture,
                    confidence,
                    repeat,
                    ..
                } => {
                    fired += 1;
                    let action = config
                        .bindings
                        .get(binding)
                        .map(|b| b.action.summary())
                        .unwrap_or_default();
                    let suffix = if repeat { " (répétition)" } else { "" };
                    println!(
                        "{at:>8.3} s  geste    {gesture} ({:.0} %) → {action}{suffix}",
                        confidence * 100.0
                    );
                }
                EngineEvent::ContinuousStarted { control } => {
                    let name = config
                        .continuous
                        .get(control)
                        .map(|c| c.name.as_str())
                        .unwrap_or("?");
                    println!("{at:>8.3} s  continu  début « {name} »");
                }
                EngineEvent::ContinuousEnded { control } => {
                    let name = config
                        .continuous
                        .get(control)
                        .map(|c| c.name.as_str())
                        .unwrap_or("?");
                    println!("{at:>8.3} s  continu  fin « {name} »");
                }
                EngineEvent::ContinuousUpdate { value, .. } if verbose => {
                    println!("{at:>8.3} s  continu  {value:?}");
                }
                _ => {}
            }
        }
    }
    let seconds = (previous_t - start) as f32 * 1e-6;
    println!(
        "Rejoué {seconds:.1} s : {transitions} transition(s), {fired} action(s) déclenchée(s)."
    );
    if header.label == "none" && seconds > 1.0 {
        // ENF-12 : déclenchements involontaires sur les sessions « none ».
        println!(
            "Session « none » : {:.1} action(s) par heure.",
            fired as f32 * 3600.0 / seconds
        );
    }
    Ok(())
}

/// `mudra record` : enregistre des points-clés pour un label (EF-60).
pub fn record(config: Arc<Config>, source: Source, request: RecordRequest) -> Result<()> {
    mudra_dataset::validate_label(&request.label)?;
    let pipeline = pipeline::start(Options {
        config,
        source,
        actions: None,
        notify: None,
        start_paused: false,
    })?;
    println!("Ouverture de la caméra…");
    let started = Instant::now();
    loop {
        let status = pipeline.status();
        if status.image_size.0 > 0
            || matches!(status.capture.state, mudra_capture::CaptureState::Streaming)
        {
            break;
        }
        if started.elapsed() > Duration::from_secs(15) {
            bail!(
                "la caméra ne démarre pas ({:?}) — Mudra tourne-t-il déjà ? Mets-le en pause.",
                status.capture.state
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    match &request.kind {
        RecordKind::Static { seconds } => println!(
            "Label « {} » · pose statique · {seconds:.0} s. Garde la pose en variant légèrement position et orientation.",
            request.label
        ),
        RecordKind::Dynamic { reps, .. } => {
            println!(
                "Label « {} » · geste dynamique · {reps} répétitions. Fais le geste à chaque bip.",
                request.label
            )
        }
    }
    pipeline.send(Command::Record(request));
    loop {
        match pipeline.events.recv_timeout(Duration::from_secs(30)) {
            Ok(PipelineEvent::Recording(event)) => match event {
                RecordEvent::Countdown {
                    seconds_left,
                    clip: None,
                } => println!("  début dans {seconds_left}…"),
                RecordEvent::Countdown {
                    seconds_left,
                    clip: Some((n, total)),
                } => {
                    println!("  répétition {n}/{total} dans {seconds_left}…")
                }
                RecordEvent::Go { clip: None } => println!("  * enregistrement"),
                RecordEvent::Go { clip: Some(_) } => println!("  * maintenant !"),
                RecordEvent::Progress { frames, fraction } => {
                    println!("  {:>3.0} % · {frames} trames", fraction * 100.0)
                }
                RecordEvent::Done { path, frames } => {
                    println!("Terminé : {frames} trames dans {}", path.display());
                    if frames == 0 {
                        println!("Attention : aucune main n'a été vue pendant l'enregistrement.");
                    }
                    return Ok(());
                }
                RecordEvent::Failed(message) => bail!("enregistrement en échec : {message}"),
            },
            Ok(PipelineEvent::Notice { message, .. }) => println!("  · {message}"),
            Ok(_) => {}
            Err(_) => bail!("le pipeline ne répond plus"),
        }
    }
}

/// Analyse « 20s », « 2m », « 1.5 » (secondes).
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let (number, factor) = if let Some(n) = text.strip_suffix("ms") {
        (n, 0.001)
    } else if let Some(n) = text.strip_suffix('s') {
        (n, 1.0)
    } else if let Some(n) = text.strip_suffix('m') {
        (n, 60.0)
    } else if let Some(n) = text.strip_suffix('h') {
        (n, 3600.0)
    } else {
        (text, 1.0)
    };
    let value: f64 = number
        .trim()
        .parse()
        .map_err(|_| format!("durée « {text} » invalide (ex. 20s, 2m)"))?;
    if !(value > 0.0 && value * factor <= 86_400.0) {
        return Err(format!("durée « {text} » hors bornes"));
    }
    Ok(Duration::from_secs_f64(value * factor))
}

/// Dossier du dataset par défaut : `./dataset`.
pub fn default_dataset_root() -> PathBuf {
    PathBuf::from("dataset")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("20s").unwrap(), Duration::from_secs(20));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_duration("300").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("1.5s").unwrap(), Duration::from_millis(1500));
        assert!(parse_duration("vite").is_err());
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("-3s").is_err());
    }
}
