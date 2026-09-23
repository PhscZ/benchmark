//! Command-line entry point.

use std::path::PathBuf;

use vk_raytracer::app::{self, AppConfig, EXIT_ERROR};
use vk_raytracer::bench::BenchConfig;
use vk_raytracer::render::{MAX_REFLECTION_BOUNCES, SAMPLE_PRESETS};

const USAGE: &str = "\
vk-raytracer - Vulkan compute GPU ray tracer (no hardware ray tracing extensions)

USAGE:
    vk-raytracer [OPTIONS]

OPTIONS:
    --validation           enable the Vulkan validation layers
    --gpu <substring>      select a physical device whose name contains <substring>
    --samples <1|16|64>    initial samples-per-pixel preset (default 64)
    --reflections <0..4>   initial reflection bounce limit (default 4)
    --exposure <float>     initial exposure (default 1.0)
    --seed <u32>           sampling seed (default 12345)
    --width <px>           render resolution width (default 1280)
    --height <px>          render resolution height (default 720)
    --output-dir <path>    directory for PNG exports and reports (default renders)
    --benchmark            run the fixed benchmark (1280x720, 64 spp, 4 bounces,
                           exposure 1.0, seed 12345), then exit
    --self-test            run the GPU verification suite, then exit
    --no-window            do not create a window (benchmark/self-test only)
    -h, --help             print this help

CONTROLS (window mode):
    left-drag              orbit the camera   (0.005 rad per pixel, pitch limited
                           to +-89 degrees, distance limited to 1.5-40 units)
    mouse wheel            zoom               (distance x 0.9 per notch)
    1 / 2 / 3              samples per pixel: 1, 16, 64 (resets accumulation)
    R / T                  reflection bounces -/+ 0..4 (resets accumulation)
    E / D                  exposure -/+  (reuses accumulated samples)
    Space                  start / restart rendering
    C                      reset camera to the default view
    P                      save PNG at render resolution (no UI overlay)
    X                      pause / resume accumulation
    B                      run the benchmark
    Esc                    quit
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Ok(ParseOutcome::Help) => {
            print!("{USAGE}");
            std::process::exit(0);
        }
        Ok(ParseOutcome::Config(config)) => {
            let code = app::run(config);
            std::process::exit(code);
        }
        Err(message) => {
            eprintln!("error: {message}\n");
            eprint!("{USAGE}");
            std::process::exit(EXIT_ERROR);
        }
    }
}

enum ParseOutcome {
    Help,
    Config(AppConfig),
}

fn parse(args: &[String]) -> Result<ParseOutcome, String> {
    let mut config = AppConfig::default();
    let mut benchmark = false;
    let mut self_test = false;
    let mut no_window = false;
    let mut benchmark_config = BenchConfig::default();
    let mut index = 0;

    while index < args.len() {
        let arg = args[index].as_str();
        let mut value = |name: &str| -> Result<String, String> {
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| format!("{name} requires a value"))
        };
        match arg {
            "-h" | "--help" => return Ok(ParseOutcome::Help),
            "--validation" => config.renderer.validation = true,
            "--gpu" => config.renderer.gpu_name_filter = Some(value("--gpu")?),
            "--samples" => {
                let samples: u32 = value("--samples")?
                    .parse()
                    .map_err(|_| "--samples expects an integer".to_string())?;
                if !SAMPLE_PRESETS.contains(&samples) {
                    return Err(format!("--samples must be one of {SAMPLE_PRESETS:?}"));
                }
                config.renderer.samples_per_pixel = Some(samples);
                benchmark_config.samples_per_pixel = samples;
            }
            "--reflections" => {
                let reflections: u32 = value("--reflections")?
                    .parse()
                    .map_err(|_| "--reflections expects an integer".to_string())?;
                if reflections > MAX_REFLECTION_BOUNCES {
                    return Err(format!("--reflections must be 0..={MAX_REFLECTION_BOUNCES}"));
                }
                config.renderer.max_reflections = Some(reflections);
                benchmark_config.max_reflections = reflections;
            }
            "--exposure" => {
                let exposure: f32 = value("--exposure")?
                    .parse()
                    .map_err(|_| "--exposure expects a number".to_string())?;
                if exposure <= 0.0 || !exposure.is_finite() {
                    return Err("--exposure must be positive".to_string());
                }
                config.renderer.exposure = Some(exposure);
                benchmark_config.exposure = exposure;
            }
            "--seed" => {
                let seed: u32 = value("--seed")?
                    .parse()
                    .map_err(|_| "--seed expects an unsigned integer".to_string())?;
                config.renderer.seed = Some(seed);
                benchmark_config.seed = seed;
            }
            "--width" => {
                let width: u32 = value("--width")?
                    .parse()
                    .map_err(|_| "--width expects an integer".to_string())?;
                config.width = width;
                benchmark_config.width = width;
            }
            "--height" => {
                let height: u32 = value("--height")?
                    .parse()
                    .map_err(|_| "--height expects an integer".to_string())?;
                config.height = height;
                benchmark_config.height = height;
            }
            "--output-dir" => {
                let dir = PathBuf::from(value("--output-dir")?);
                config.output_dir = dir.clone();
                benchmark_config.output_dir = dir;
            }
            "--benchmark" => benchmark = true,
            "--self-test" => self_test = true,
            "--no-window" => no_window = true,
            other => return Err(format!("unknown argument: {other}")),
        }
        index += 1;
    }

    config.self_test = self_test;
    config.headless = no_window;
    if benchmark {
        benchmark_config.validate().map_err(|e| e.to_string())?;
        config.benchmark = Some(benchmark_config);
    }
    if no_window && !benchmark && !self_test {
        return Err("--no-window requires --benchmark and/or --self-test".to_string());
    }
    Ok(ParseOutcome::Config(config))
}

