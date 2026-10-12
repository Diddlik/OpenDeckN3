#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 system monitor plugin: shows CPU, RAM, GPU, temperatures,
//! network and disk values on a key, as a graph or as a number. GPU values
//! need NVIDIA's NVML library (see `docs/PLUGIN_SYSMON.md`).

use std::{
    collections::{HashMap, VecDeque, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    io::Cursor,
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{ImageFormat, Rgb, RgbImage};
use n3_plugin_sdk::{Contexts, Host, Instance};
use nvml_wrapper::{Nvml, enum_wrappers::device::TemperatureSensor};
use serde_json::{Value, json};
use sysinfo::{Components, Disks, Networks, System};
use tokio::sync::mpsc;

const TICK: Duration = Duration::from_secs(1);
const HISTORY_LEN: usize = 60;
const KEY_SIZE: u32 = 144;
const PLOT_X: u32 = 4;
const PLOT_W: u32 = 136;
const PLOT_BOTTOM: u32 = 140;
const PLOT_H: u32 = 100;
const HEADER_Y: u32 = 6;
const GLYPH_SCALE: u32 = 2;
const GLYPH_ADVANCE: u32 = 4 * GLYPH_SCALE;
const BACKGROUND: Rgb<u8> = Rgb([0x17, 0x18, 0x1c]);
const HEADER_COLOR: Rgb<u8> = Rgb([0x9a, 0xa4, 0xb2]);
const ERROR_TITLE: &str = "–";
const MIB: f64 = 1024.0 * 1024.0;
const KIB: f64 = 1024.0;

#[derive(Clone, Copy)]
enum Metric {
    Cpu,
    Ram,
    Gpu,
    Vram,
    GpuTemp,
    CpuTemp,
    NetDown,
    NetUp,
    Disk,
}

#[derive(Clone, Copy)]
enum Unit {
    Percent,
    Celsius,
    Rate,
}

impl Metric {
    fn parse(id: &str) -> Option<Self> {
        Some(match id {
            "cpu" => Self::Cpu,
            "ram" => Self::Ram,
            "gpu" => Self::Gpu,
            "vram" => Self::Vram,
            "gputemp" => Self::GpuTemp,
            "cputemp" => Self::CpuTemp,
            "netdown" => Self::NetDown,
            "netup" => Self::NetUp,
            "disk" => Self::Disk,
            _ => return None,
        })
    }

    /// Short name drawn into the graph header.
    fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Ram => "RAM",
            Self::Gpu => "GPU",
            Self::Vram => "VRAM",
            Self::GpuTemp => "GPU TEMP",
            Self::CpuTemp => "CPU TEMP",
            Self::NetDown => "NET DOWN",
            Self::NetUp => "NET UP",
            Self::Disk => "DISK",
        }
    }

    fn unit(self) -> Unit {
        match self {
            Self::Cpu | Self::Ram | Self::Gpu | Self::Vram | Self::Disk => Unit::Percent,
            Self::GpuTemp | Self::CpuTemp => Unit::Celsius,
            Self::NetDown | Self::NetUp => Unit::Rate,
        }
    }

    fn default_color(self) -> Rgb<u8> {
        match self {
            Self::Cpu => Rgb([0x5b, 0xc4, 0xff]),
            Self::Ram => Rgb([0xb3, 0x88, 0xff]),
            Self::Gpu => Rgb([0x3f, 0xb9, 0x50]),
            Self::Vram => Rgb([0xd2, 0x9a, 0x3c]),
            Self::GpuTemp => Rgb([0xff, 0x7b, 0x72]),
            Self::CpuTemp => Rgb([0xff, 0x8f, 0x6b]),
            Self::NetDown => Rgb([0x5f, 0xd3, 0x8a]),
            Self::NetUp => Rgb([0xf5, 0xb8, 0x41]),
            Self::Disk => Rgb([0x7d, 0xd3, 0xd3]),
        }
    }
}

/// One sampling pass. `None` means the value is not available on this system.
#[derive(Default)]
struct Readings {
    cpu: Option<f64>,
    ram: Option<f64>,
    gpu: Option<f64>,
    vram: Option<f64>,
    gpu_temp: Option<f64>,
    cpu_temp: Option<f64>,
    net_down: Option<f64>,
    net_up: Option<f64>,
    /// Used percent per mount point.
    disks: Vec<(String, f64)>,
}

impl Readings {
    fn value(&self, metric: Metric, disk: &str) -> Option<f64> {
        match metric {
            Metric::Cpu => self.cpu,
            Metric::Ram => self.ram,
            Metric::Gpu => self.gpu,
            Metric::Vram => self.vram,
            Metric::GpuTemp => self.gpu_temp,
            Metric::CpuTemp => self.cpu_temp,
            Metric::NetDown => self.net_down,
            Metric::NetUp => self.net_up,
            Metric::Disk => self
                .disks
                .iter()
                .find(|(mount, _)| mount == disk)
                .map(|(_, used)| *used),
        }
    }
}

/// Owns the blocking system handles. It moves into `spawn_blocking` for every
/// sample and comes back with the readings.
struct Sampler {
    system: System,
    components: Components,
    disks: Disks,
    networks: Networks,
    nvml: Option<Nvml>,
    last_net: Instant,
}

impl Sampler {
    fn new(nvml: Option<Nvml>) -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        Self {
            system,
            components: Components::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            networks: Networks::new_with_refreshed_list(),
            nvml,
            last_net: Instant::now(),
        }
    }

    fn sample(&mut self) -> Readings {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.components.refresh(true);
        self.disks.refresh(true);
        self.networks.refresh(true);

        let elapsed = self.last_net.elapsed().as_secs_f64().max(1e-3);
        self.last_net = Instant::now();
        let (down, up) = self.networks.iter().fold((0, 0), |(down, up), (_, net)| {
            (down + net.received(), up + net.transmitted())
        });

        let device = self
            .nvml
            .as_ref()
            .and_then(|nvml| nvml.device_by_index(0).ok());
        let gpu = device.as_ref().and_then(|d| d.utilization_rates().ok());
        let memory = device.as_ref().and_then(|d| d.memory_info().ok());
        let gpu_temp = device
            .as_ref()
            .and_then(|d| d.temperature(TemperatureSensor::Gpu).ok());

        Readings {
            cpu: Some(f64::from(self.system.global_cpu_usage())),
            ram: percent(self.system.used_memory(), self.system.total_memory()),
            gpu: gpu.map(|u| f64::from(u.gpu)),
            vram: memory.and_then(|m| percent(m.used, m.total)),
            gpu_temp: gpu_temp.map(f64::from),
            cpu_temp: self
                .components
                .iter()
                .filter(|c| is_cpu_sensor(c.label()))
                .filter_map(|c| c.temperature())
                .reduce(f32::max)
                .map(f64::from),
            net_down: Some(down as f64 / elapsed),
            net_up: Some(up as f64 / elapsed),
            disks: self
                .disks
                .iter()
                .filter_map(|d| {
                    let used = d.total_space().checked_sub(d.available_space())?;
                    let mount = d.mount_point().to_string_lossy().into_owned();
                    Some((mount, percent(used, d.total_space())?))
                })
                .collect(),
        }
    }
}

fn percent(used: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| used as f64 / total as f64 * 100.0)
}

/// Temperature sensors that belong to the CPU; other sensors are skipped.
fn is_cpu_sensor(label: &str) -> bool {
    let label = label.to_lowercase();
    ["cpu", "core", "package", "tctl", "tdie"]
        .iter()
        .any(|word| label.contains(word))
}

enum Msg {
    Host(Value),
    Sampled(Box<Sampler>, Readings),
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    contexts: Contexts,
    /// `None` while a sample is running on the blocking pool.
    sampler: Option<Box<Sampler>>,
    /// Last values per key, oldest first; `None` is a gap.
    history: HashMap<String, VecDeque<Option<f64>>>,
    /// Hash of the graph last sent per key; `None` means the default image.
    images: HashMap<String, Option<u64>>,
}

impl Plugin {
    fn new(host: Host, tx: mpsc::UnboundedSender<Msg>, sampler: Sampler) -> Self {
        Self {
            host,
            tx,
            contexts: Contexts::default(),
            sampler: Some(Box::new(sampler)),
            history: HashMap::new(),
            images: HashMap::new(),
        }
    }

    fn on_host(&mut self, msg: Value) {
        if let Some(context) = self.contexts.apply(&msg) {
            self.history.remove(&context);
            self.images.remove(&context);
            return;
        }
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "keyDown" => self.press(&context),
            "sendToPlugin" => self.on_request(msg["payload"].clone()),
            _ => {}
        }
    }

    /// Opens the Task Manager on Windows; nothing happens on Linux.
    fn press(&self, context: &str) {
        if self.contexts.get(context).is_none() {
            return;
        }
        if cfg!(windows)
            && let Err(err) = std::process::Command::new("taskmgr.exe").spawn()
        {
            self.host.alert(context);
            self.host.log(format!(
                "Task-Manager konnte nicht gestartet werden: {err:#}"
            ));
        }
    }

    fn on_request(&self, req: Value) {
        let host = self.host.clone();
        tokio::spawn(async move {
            let options = match req["request"].as_str() {
                Some("disks") => tokio::task::spawn_blocking(disk_options)
                    .await
                    .context("Laufwerke nicht abrufbar"),
                other => Err(anyhow!("unbekannte Anfrage {other:?}")),
            };
            host.reply_options(&req, options);
        });
    }

    fn on_tick(&mut self) {
        if !self.contexts.any("metric") {
            return;
        }
        let Some(mut sampler) = self.sampler.take() else {
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let sampled = tokio::task::spawn_blocking(move || {
                let readings = sampler.sample();
                (sampler, readings)
            })
            .await;
            if let Ok((sampler, readings)) = sampled {
                tx.send(Msg::Sampled(sampler, readings)).ok();
            }
        });
    }

    fn on_sampled(&mut self, sampler: Box<Sampler>, readings: Readings) {
        self.sampler = Some(sampler);
        let instances: Vec<(String, Instance)> = self
            .contexts
            .iter()
            .map(|(context, inst)| (context.clone(), inst.clone()))
            .collect();
        for (context, inst) in instances {
            if inst.kind() != "metric" {
                continue;
            }
            let Some(metric) = Metric::parse(&inst.text_or("metric", "cpu")) else {
                continue;
            };
            let value = readings.value(metric, &inst.text("disk"));
            let history = self.history.entry(context.clone()).or_default();
            push(history, value);
            let title = format_value(metric, value);
            self.contexts.show(&self.host, &context, 0, &title);
            if inst.text_or("style", "graph") == "graph" {
                self.send_graph(&context, &inst, metric);
            } else if self.images.insert(context.clone(), None) != Some(None) {
                self.set_image(&context, None);
            }
        }
        self.history
            .retain(|context, _| self.contexts.get(context).is_some());
        self.images
            .retain(|context, _| self.contexts.get(context).is_some());
    }

    fn send_graph(&mut self, context: &str, inst: &Instance, metric: Metric) {
        let history = &self.history[context];
        let color = parse_color(&inst.text("color")).unwrap_or(metric.default_color());
        let graph = draw_graph(metric.label(), history, scale(metric, history), color);
        let hash = hash_bytes(graph.as_raw());
        if self.images.get(context) == Some(&Some(hash)) {
            return;
        }
        match png_data_url(&graph) {
            Ok(url) => {
                self.set_image(context, Some(url));
                self.images.insert(context.to_owned(), Some(hash));
            }
            Err(err) => self.host.log(format!("Grafik nicht erzeugbar: {err:#}")),
        }
    }

    fn set_image(&self, context: &str, image: Option<String>) {
        self.host.send(json!({
            "event": "setImage",
            "context": context,
            "payload": { "image": image },
        }));
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Host(msg) => self.on_host(msg),
            Msg::Sampled(sampler, readings) => self.on_sampled(sampler, readings),
        }
    }
}

fn push(history: &mut VecDeque<Option<f64>>, value: Option<f64>) {
    if history.len() == HISTORY_LEN {
        history.pop_front();
    }
    history.push_back(value);
}

/// Upper end of the graph: fixed for percentages and temperatures, the
/// largest value of the window for rates.
fn scale(metric: Metric, history: &VecDeque<Option<f64>>) -> f64 {
    match metric.unit() {
        Unit::Rate => history.iter().flatten().copied().fold(1.0, f64::max),
        Unit::Percent | Unit::Celsius => 100.0,
    }
}

fn format_value(metric: Metric, value: Option<f64>) -> String {
    let Some(value) = value else {
        return ERROR_TITLE.to_owned();
    };
    match metric.unit() {
        Unit::Percent => format!("{value:.0} %"),
        Unit::Celsius => format!("{value:.0} °C"),
        Unit::Rate => format_rate(value),
    }
}

fn format_rate(bytes_per_s: f64) -> String {
    let (number, unit) = if bytes_per_s >= MIB {
        (format!("{:.1}", bytes_per_s / MIB), "MB/s")
    } else if bytes_per_s >= KIB {
        (format!("{:.0}", bytes_per_s / KIB), "KB/s")
    } else {
        (format!("{bytes_per_s:.0}"), "B/s")
    };
    format!("{} {unit}", number.replace('.', ","))
}

/// `#rrggbb` as set in the action's color field.
fn parse_color(text: &str) -> Option<Rgb<u8>> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some(Rgb([channel(0)?, channel(2)?, channel(4)?]))
}

fn draw_graph(
    header: &str,
    history: &VecDeque<Option<f64>>,
    scale: f64,
    color: Rgb<u8>,
) -> RgbImage {
    let mut img = RgbImage::from_pixel(KEY_SIZE, KEY_SIZE, BACKGROUND);
    draw_text(&mut img, PLOT_X + 2, HEADER_Y, header, HEADER_COLOR);
    // Newest value on the right; the window fills in from the right edge.
    let missing = HISTORY_LEN - history.len();
    for x in 0..PLOT_W {
        let slot = x as usize * HISTORY_LEN / PLOT_W as usize;
        let Some(value) = slot
            .checked_sub(missing)
            .and_then(|i| history.get(i).copied().flatten())
        else {
            continue;
        };
        let height = ((value / scale).clamp(0.0, 1.0) * f64::from(PLOT_H)).round() as u32;
        for y in PLOT_BOTTOM - height..PLOT_BOTTOM {
            img.put_pixel(PLOT_X + x, y, color);
        }
    }
    img
}

/// Draws `text` with a 3×5 pixel font, scaled up.
fn draw_text(img: &mut RgbImage, x: u32, y: u32, text: &str, color: Rgb<u8>) {
    for (i, c) in text.chars().enumerate() {
        let origin_x = x + i as u32 * GLYPH_ADVANCE;
        for (row, line) in glyph(c).iter().enumerate() {
            for (col, bit) in line.bytes().enumerate() {
                if bit != b'#' {
                    continue;
                }
                for dy in 0..GLYPH_SCALE {
                    for dx in 0..GLYPH_SCALE {
                        let px = origin_x + col as u32 * GLYPH_SCALE + dx;
                        let py = y + row as u32 * GLYPH_SCALE + dy;
                        img.put_pixel(px, py, color);
                    }
                }
            }
        }
    }
}

/// Glyphs for the metric labels only; other characters are blank.
fn glyph(c: char) -> [&'static str; 5] {
    match c {
        'A' => [".#.", "#.#", "###", "#.#", "#.#"],
        'C' => [".##", "#..", "#..", "#..", ".##"],
        'D' => ["##.", "#.#", "#.#", "#.#", "##."],
        'E' => ["###", "#..", "##.", "#..", "###"],
        'G' => [".##", "#..", "#.#", "#.#", ".##"],
        'I' => ["###", ".#.", ".#.", ".#.", "###"],
        'K' => ["#.#", "#.#", "##.", "#.#", "#.#"],
        'M' => ["#.#", "###", "###", "#.#", "#.#"],
        'N' => ["##.", "#.#", "#.#", "#.#", "#.#"],
        'O' => [".#.", "#.#", "#.#", "#.#", ".#."],
        'P' => ["##.", "#.#", "##.", "#..", "#.."],
        'R' => ["##.", "#.#", "##.", "#.#", "#.#"],
        'S' => [".##", "#..", ".#.", "..#", "##."],
        'T' => ["###", ".#.", ".#.", ".#.", ".#."],
        'U' => ["#.#", "#.#", "#.#", "#.#", "###"],
        'V' => ["#.#", "#.#", "#.#", "#.#", ".#."],
        'W' => ["#.#", "#.#", "###", "###", "#.#"],
        _ => ["...", "...", "...", "...", "..."],
    }
}

fn png_data_url(img: &RgbImage) -> anyhow::Result<String> {
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(png)))
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Mount point and label for the disk dropdown.
fn disk_options() -> Vec<(String, String)> {
    Disks::new_with_refreshed_list()
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| {
            let mount = d.mount_point().to_string_lossy().into_owned();
            (mount.clone(), mount)
        })
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let nvml = match Nvml::init() {
        Ok(nvml) => Some(nvml),
        Err(err) => {
            host.log(format!("GPU-Werte nicht verfügbar (NVML): {err}"));
            None
        }
    };
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut plugin = Plugin::new(host, tx, Sampler::new(nvml));
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.handle(Msg::Host(msg)),
                None => break,
            },
            Some(msg) = rx.recv() => plugin.handle(msg),
            _ = tick.tick() => plugin.on_tick(),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_last_60_values() {
        let mut history = VecDeque::new();
        for i in 0..70 {
            push(&mut history, Some(f64::from(i)));
        }
        assert_eq!(history.len(), HISTORY_LEN);
        assert_eq!(history.front(), Some(&Some(10.0)));
        assert_eq!(history.back(), Some(&Some(69.0)));
    }

    #[test]
    fn values_are_formatted_for_the_key() {
        assert_eq!(format_value(Metric::Cpu, Some(42.4)), "42 %");
        assert_eq!(format_value(Metric::GpuTemp, Some(61.0)), "61 °C");
        assert_eq!(format_value(Metric::NetDown, Some(3.2 * MIB)), "3,2 MB/s");
        assert_eq!(format_value(Metric::NetUp, Some(512.0 * KIB)), "512 KB/s");
        assert_eq!(format_value(Metric::NetUp, Some(12.0)), "12 B/s");
        assert_eq!(format_value(Metric::Ram, None), ERROR_TITLE);
    }

    #[test]
    fn rate_scale_follows_the_window() {
        let history: VecDeque<Option<f64>> = [Some(5.0), None, Some(20.0)].into();
        assert_eq!(scale(Metric::NetDown, &history), 20.0);
        assert_eq!(scale(Metric::Cpu, &history), 100.0);
        assert_eq!(scale(Metric::NetUp, &VecDeque::new()), 1.0);
    }

    #[test]
    fn color_must_be_hex() {
        assert_eq!(parse_color("#ff8000"), Some(Rgb([0xff, 0x80, 0x00])));
        assert_eq!(parse_color("ff8000"), None);
        assert_eq!(parse_color("#ff80"), None);
        assert_eq!(parse_color("#ggg000"), None);
    }

    #[test]
    fn graph_has_key_size_and_survives_extremes() {
        let mut history = VecDeque::new();
        let color = Metric::Cpu.default_color();
        assert_eq!(
            draw_graph("CPU", &history, 100.0, color).dimensions(),
            (KEY_SIZE, KEY_SIZE)
        );
        push(&mut history, Some(0.0));
        push(&mut history, None);
        push(&mut history, Some(100.0));
        let img = draw_graph("CPU TEMP", &history, 100.0, color);
        assert_eq!(img.dimensions(), (KEY_SIZE, KEY_SIZE));
        assert_eq!(*img.get_pixel(PLOT_X + PLOT_W - 1, PLOT_BOTTOM - 1), color);
        let full: VecDeque<Option<f64>> = (0..HISTORY_LEN).map(|_| Some(250.0)).collect();
        draw_graph("NET UP", &full, 1.0, color);
    }

    #[test]
    fn cpu_sensors_are_recognized() {
        assert!(is_cpu_sensor("Core 0"));
        assert!(is_cpu_sensor("k10temp Tctl"));
        assert!(!is_cpu_sensor("GPU"));
    }

    #[test]
    fn percent_needs_a_total() {
        assert_eq!(percent(50, 200), Some(25.0));
        assert_eq!(percent(0, 0), None);
    }

    #[test]
    fn graph_is_a_png_data_url() {
        let url = png_data_url(&draw_graph("RAM", &VecDeque::new(), 100.0, BACKGROUND)).unwrap();
        let png = STANDARD
            .decode(url.strip_prefix("data:image/png;base64,").unwrap())
            .unwrap();
        let decoded = image::load_from_memory_with_format(&png, ImageFormat::Png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (KEY_SIZE, KEY_SIZE));
    }
}
