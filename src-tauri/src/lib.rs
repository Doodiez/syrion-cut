mod engine;

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::{path::BaseDirectory, AppHandle, Manager};

use engine::{Engine, Finish, Model};

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff"];
const OUTPUT_FOLDER: &str = "no-bg";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Options {
    /// 0..=100 haze cleanup strength.
    cleanup: u32,
    /// Remove background colour fringing on soft edges.
    fix_edges: bool,
    /// 1 = off, 2 or 4 = AI upscale.
    upscale: u32,
    /// Edge shrink in px (0..=10).
    shrink: u32,
    /// Edge soften in px (0..=10).
    soften: u32,
    /// Draw a sticker-style outline.
    outline: bool,
    /// 1..=20; thickness as a fraction of the image size (size/400 of the longest side).
    outline_size: u32,
    /// "#rrggbb"
    outline_color: String,
    /// Trim transparent space around the subject.
    auto_crop: bool,
    /// Pixels of empty space to keep around the subject when cropping.
    padding: u32,
    /// Where to save. None = a "no-bg" folder next to each original.
    output_dir: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Done {
    output: String,
    preview: String,
    width: u32,
    height: u32,
    orig_rect: [i64; 4],
    millis: u128,
}

fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn collect(p: &Path, out: &mut Vec<String>) {
    if p.is_dir() {
        // Never re-process our own results.
        if p.file_name().and_then(|n| n.to_str()) == Some(OUTPUT_FOLDER) {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(p) {
            let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            children.sort();
            for c in children {
                collect(&c, out);
            }
        }
    } else if is_image(p) {
        out.push(p.to_string_lossy().into_owned());
    }
}

/// Turns dropped files and folders into a flat list of image files.
#[tauri::command]
fn expand_paths(paths: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for p in paths {
        collect(Path::new(&p), &mut out);
    }
    out.dedup();
    out
}

fn outline_px(img: &image::RgbaImage, size: u32) -> u32 {
    let longest = img.width().max(img.height()) as f32;
    ((longest * size.clamp(1, 20) as f32 / 400.0).round() as u32).max(1)
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

fn data_url(img: &image::DynamicImage, max: u32) -> Result<String, String> {
    let thumb = img.thumbnail(max, max);
    let mut bytes = Vec::new();
    thumb
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// Small preview of an original image for the queue.
#[tauri::command]
async fn thumbnail(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let img = image::open(&path).map_err(|e| format!("Can't open image: {e}"))?;
        data_url(&img, 160)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve("resources", BaseDirectory::Resource)
        .map_err(|e| e.to_string())
}

/// Loads an AI model and reports whether it runs on the GPU or CPU.
#[tauri::command]
async fn warm_up(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let engine = app.state::<Engine>();
        engine.device(&models_dir(&app)?, Model::Best)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn remove_background(app: AppHandle, path: String, options: Options) -> Result<Done, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let src = Path::new(&path);
        let img = image::open(src).map_err(|e| format!("Can't open image: {e}"))?;

        let engine = app.state::<Engine>();
        let finish = Finish {
            cleanup: options.cleanup,
            fix_edges: options.fix_edges,
            upscale: options.upscale,
            shrink: options.shrink.min(10),
            soften: options.soften.min(10),
        };
        let mut cut = engine.cut_out(&models_dir(&app)?, Model::Best, &img, finish)?;
        // Where the whole original photo lands in the saved file: [x, y, w, h].
        let mut orig_rect = [0i64, 0, cut.width() as i64, cut.height() as i64];

        if options.auto_crop {
            // Padding is in original-image pixels; keep it proportional after upscaling.
            let scale = cut.width() as f32 / img.width() as f32;
            let (c, x0, y0) = engine::crop_to_subject(cut, (options.padding as f32 * scale).round() as u32);
            cut = c;
            orig_rect[0] -= x0 as i64;
            orig_rect[1] -= y0 as i64;
        }
        if options.outline {
            let thickness = outline_px(&cut, options.outline_size);
            orig_rect[0] += engine::outline_pad(thickness) as i64;
            orig_rect[1] += engine::outline_pad(thickness) as i64;
            cut = engine::add_outline(
                cut,
                thickness,
                parse_hex(&options.outline_color).unwrap_or([255, 255, 255]),
            );
        }

        let out_dir = match &options.output_dir {
            Some(d) => PathBuf::from(d),
            None => src.parent().unwrap_or(Path::new(".")).join(OUTPUT_FOLDER),
        };
        std::fs::create_dir_all(&out_dir).map_err(|e| format!("Can't create output folder: {e}"))?;
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
        let out_path = out_dir.join(format!("{stem}.png"));

        let (width, height) = cut.dimensions();
        let cut = image::DynamicImage::ImageRgba8(cut);
        cut.save_with_format(&out_path, image::ImageFormat::Png)
            .map_err(|e| format!("Can't save result: {e}"))?;

        Ok(Done {
            output: out_path.to_string_lossy().into_owned(),
            preview: data_url(&cut, 900)?,
            width,
            height,
            orig_rect,
            millis: started.elapsed().as_millis(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Headless mode for scripts/testing:
/// `syrion-cut --cut <input> <output.png> [--crop] [--cleanup N] [--no-fix-edges]`.
fn cli(args: &[String]) -> Option<i32> {
    let i = args.iter().position(|a| a == "--cut")?;
    let (Some(input), Some(output)) = (args.get(i + 1), args.get(i + 2)) else {
        eprintln!("usage: syrion-cut --cut <input> <output.png> [--crop] [--cleanup N] [--no-fix-edges]");
        return Some(2);
    };
    let has = |f: &str| args.iter().any(|a| a == f);
    let model = Model::Best;
    let cleanup = args
        .iter()
        .position(|a| a == "--cleanup")
        .and_then(|j| args.get(j + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let num = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|j| args.get(j + 1))
            .and_then(|v| v.parse::<u32>().ok())
    };
    let finish = Finish {
        cleanup,
        fix_edges: !has("--no-fix-edges"),
        upscale: num("--upscale").unwrap_or(1),
        shrink: num("--shrink").unwrap_or(0),
        soften: num("--soften").unwrap_or(0),
    };
    let outline = num("--outline");
    let dir = std::env::current_exe().ok()?.parent()?.join("resources");
    let result = (|| -> Result<String, String> {
        let img = image::open(input).map_err(|e| e.to_string())?;
        let engine = Engine::default();
        let device = engine.device(&dir, model)?;
        let started = std::time::Instant::now();
        let mut cut = engine.cut_out(&dir, model, &img, finish)?;
        if args.iter().any(|a| a == "--crop") {
            cut = engine::crop_to_subject(cut, 20).0;
        }
        if let Some(size) = outline {
            let thickness = outline_px(&cut, size);
            cut = engine::add_outline(cut, thickness, [255, 255, 255]);
        }
        cut.save_with_format(output, image::ImageFormat::Png).map_err(|e| e.to_string())?;
        Ok(format!("{device} {}ms {}x{}", started.elapsed().as_millis(), cut.width(), cut.height()))
    })();
    match result {
        Ok(msg) => { println!("ok {msg}"); Some(0) }
        Err(e) => { eprintln!("error: {e}"); Some(1) }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = cli(&args) {
        std::process::exit(code);
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Engine::default())
        .invoke_handler(tauri::generate_handler![
            expand_paths,
            thumbnail,
            warm_up,
            remove_background
        ])
        .run(tauri::generate_context!())
        .expect("error while running Syrion Cut");
}
