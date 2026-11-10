//! Background removal with segmentation models via ONNX Runtime.
//! Tries the GPU (DirectML) first and falls back to the CPU.
//!
//! Models:
//!  - BiRefNet (MIT licence): background removal.
//!  - Real-ESRGAN x4plus (BSD-3): AI upscale.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use image::{imageops::FilterType, DynamicImage, GenericImageView, GrayImage, RgbaImage};
use ort::environment::Environment;
use ort::ep::DirectML;
use ort::session::Session;
use ort::value::Tensor;

const SIZE: u32 = 1024;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Model {
    /// BiRefNet background removal.
    Best,
    /// Real-ESRGAN x4plus super-resolution.
    Upscaler,
}

/// Longest side we'll upscale to; bigger outputs get unwieldy for design apps.
const MAX_UPSCALED_SIDE: u32 = 8000;

struct Spec {
    file: &'static str,
    input: &'static str,
    output: &'static str,
    mean: [f32; 3],
    std: [f32; 3],
    /// Output is raw logits that need a sigmoid to become 0..1.
    sigmoid: bool,
}

impl Model {
    fn file(self) -> &'static str {
        match self {
            Model::Upscaler => "real_esrgan_x4plus.onnx",
            m => m.spec().file,
        }
    }

    fn spec(self) -> Spec {
        match self {
            Model::Upscaler => unreachable!("upscaler has no segmentation spec"),
            Model::Best => Spec {
                file: "birefnet.onnx",
                input: "input_image",
                output: "output_image",
                mean: [0.485, 0.456, 0.406],
                std: [0.229, 0.224, 0.225],
                sigmoid: true,
            },
        }
    }
}

/// How the mask is turned into the final cut-out.
#[derive(Clone, Copy)]
pub struct Finish {
    /// 0..=100. Removes faint leftover haze/shadow and firms up the subject.
    pub cleanup: u32,
    /// Remove background colour bleeding into soft edges (halos).
    pub fix_edges: bool,
    /// 1 = off, 2 or 4 = AI upscale factor.
    pub upscale: u32,
    /// Pull the edge inward by this many pixels (of the original photo) to drop fringes.
    pub shrink: u32,
    /// Feather the edge by this many pixels (of the original photo).
    pub soften: u32,
}

struct Loaded {
    session: Session,
    device: String,
}

#[derive(Default)]
pub struct Engine {
    env: Mutex<Option<Environment>>,
    sessions: Mutex<HashMap<Model, Loaded>>,
}

fn gpu_session(env: &Environment, model: &Path) -> ort::Result<Session> {
    Ok(Session::builder(env)?
        .with_memory_pattern(false)?
        .with_execution_providers([DirectML::default().build().error_on_failure()])?
        .commit_from_file(model)?)
}

fn cpu_session(env: &Environment, model: &Path) -> ort::Result<Session> {
    Ok(Session::builder(env)?.commit_from_file(model)?)
}

impl Engine {
    fn with<T>(
        &self,
        models_dir: &Path,
        model: Model,
        f: impl FnOnce(&mut Loaded) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut sessions = self.sessions.lock().map_err(|_| "AI engine crashed; restart the app".to_string())?;
        if !sessions.contains_key(&model) {
            let mut env_guard = self.env.lock().map_err(|_| "AI engine crashed; restart the app".to_string())?;
            if env_guard.is_none() {
                *env_guard = Some(
                    ort::init()
                        .with_name("syrion-cut")
                        .build()
                        .map_err(|e| format!("Couldn't start AI runtime: {e}"))?,
                );
            }
            let env = env_guard.as_ref().unwrap();
            let path: PathBuf = models_dir.join(model.file());
            let loaded = match gpu_session(env, &path) {
                Ok(session) => Loaded { session, device: "GPU".into() },
                Err(gpu_err) => {
                    eprintln!("GPU unavailable for {model:?}, using CPU: {gpu_err}");
                    let session = cpu_session(env, &path).map_err(|e| format!("Couldn't load AI model: {e}"))?;
                    Loaded { session, device: "CPU".into() }
                }
            };
            sessions.insert(model, loaded);
        }
        f(sessions.get_mut(&model).unwrap())
    }

    pub fn device(&self, models_dir: &Path, model: Model) -> Result<String, String> {
        self.with(models_dir, model, |l| Ok(l.device.clone()))
    }

    /// Returns the image with its background made transparent.
    pub fn cut_out(&self, models_dir: &Path, model: Model, img: &DynamicImage, finish: Finish) -> Result<RgbaImage, String> {
        let spec = model.spec();
        let (w, h) = img.dimensions();

        // 1. Squash to 1024x1024 and normalise, channels-first.
        let small = img.resize_exact(SIZE, SIZE, FilterType::Triangle).to_rgb8();
        let plane = (SIZE * SIZE) as usize;
        let mut input = vec![0f32; 3 * plane];
        for (i, px) in small.pixels().enumerate() {
            for c in 0..3 {
                input[c * plane + i] = (px[c] as f32 / 255.0 - spec.mean[c]) / spec.std[c];
            }
        }
        let tensor = Tensor::from_array(([1usize, 3, SIZE as usize, SIZE as usize], input))
            .map_err(|e| e.to_string())?;

        // 2. Run the model; its output is the foreground mask.
        let mask: Vec<f32> = self.with(models_dir, model, |l| {
            let outputs = l
                .session
                .run(ort::inputs![spec.input => tensor])
                .map_err(|e| format!("AI processing failed: {e}"))?;
            let (_, data) = outputs[spec.output]
                .try_extract_tensor::<f32>()
                .map_err(|e| e.to_string())?;
            Ok(data.to_vec())
        })?;

        // 3. Mask -> 0-255, scaled back to the original size.
        let mask_px: Vec<u8> = mask
            .iter()
            .map(|&v| {
                let v = if spec.sigmoid { 1.0 / (1.0 + (-v).exp()) } else { v };
                (v * 255.0).round().clamp(0.0, 255.0) as u8
            })
            .collect();
        let mask_img = GrayImage::from_raw(SIZE, SIZE, mask_px).ok_or("bad mask size")?;
        let mut alpha = image::imageops::resize(&mask_img, w, h, FilterType::CatmullRom);

        // 4. Haze cleanup: push faint alpha to 0 and near-solid alpha to 255.
        if finish.cleanup > 0 {
            let s = finish.cleanup.min(100) as f32 / 100.0;
            let lo = 110.0 * s; // below this -> fully transparent
            let hi = 255.0 - 60.0 * s; // above this -> fully solid
            for p in alpha.pixels_mut() {
                let a = p[0] as f32;
                p[0] = (((a - lo) / (hi - lo)) * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }

        // 5. Optional AI upscale: enlarge the photo with Real-ESRGAN and the mask to match.
        let mut out = img.to_rgba8();
        let scale = if finish.upscale > 1 {
            (finish.upscale as f32).min(MAX_UPSCALED_SIDE as f32 / w.max(h) as f32)
        } else {
            1.0
        };
        if scale > 1.05 {
            let (tw, th) = ((w as f32 * scale).round() as u32, (h as f32 * scale).round() as u32);
            let mut rgb = self.esrgan(models_dir, &img.to_rgb8())?;
            if rgb.dimensions() != (tw, th) {
                rgb = image::imageops::resize(&rgb, tw, th, FilterType::Lanczos3);
            }
            let orig_alpha = GrayImage::from_fn(w, h, |x, y| image::Luma([out.get_pixel(x, y)[3]]));
            let orig_alpha = image::imageops::resize(&orig_alpha, tw, th, FilterType::CatmullRom);
            alpha = image::imageops::resize(&alpha, tw, th, FilterType::CatmullRom);
            out = RgbaImage::from_fn(tw, th, |x, y| {
                let p = rgb.get_pixel(x, y);
                image::Rgba([p[0], p[1], p[2], orig_alpha.get_pixel(x, y)[0]])
            });
        }

        // 6. Edge refinement, sized relative to the original photo so it looks the same after upscaling.
        let px_scale = alpha.width() as f32 / w as f32;
        if finish.shrink > 0 {
            let r = (finish.shrink as f32 * px_scale).round().max(1.0) as usize;
            erode(&mut alpha, r);
        }
        if finish.soften > 0 {
            let r = (finish.soften as f32 * px_scale).round().max(1.0) as usize;
            feather(&mut alpha, r);
        }

        // 7. Combine with the photo (respecting any transparency already there).
        for (px, m) in out.pixels_mut().zip(alpha.pixels()) {
            px[3] = ((px[3] as u16 * m[0] as u16) / 255) as u8;
        }
        if finish.fix_edges {
            decontaminate(&mut out);
        }
        Ok(out)
    }

    /// 4x super-resolution with Real-ESRGAN. The model takes fixed 128px tiles, so the image
    /// is processed in overlapping tiles and only each tile's centre is kept (no visible seams).
    fn esrgan(&self, models_dir: &Path, rgb: &image::RgbImage) -> Result<image::RgbImage, String> {
        const T: u32 = 128; // tile size the model expects
        const P: u32 = 12; // overlap discarded on each side
        const S: u32 = T - 2 * P; // useful step
        const X: u32 = 4; // model scale
        const O: u32 = T * X; // output tile size
        let (w, h) = rgb.dimensions();
        let mut out = image::RgbImage::new(w * X, h * X);
        self.with(models_dir, Model::Upscaler, |l| {
            let plane = (T * T) as usize;
            let mut input = vec![0f32; 3 * plane];
            let mut ty = 0;
            while ty < h {
                let mut tx = 0;
                while tx < w {
                    for y in 0..T {
                        let sy = (ty as i64 + y as i64 - P as i64).clamp(0, h as i64 - 1) as u32;
                        for x in 0..T {
                            let sx = (tx as i64 + x as i64 - P as i64).clamp(0, w as i64 - 1) as u32;
                            let p = rgb.get_pixel(sx, sy);
                            let i = (y * T + x) as usize;
                            for c in 0..3 {
                                input[c * plane + i] = p[c] as f32 / 255.0;
                            }
                        }
                    }
                    let tensor = Tensor::from_array(([1usize, 3, T as usize, T as usize], input.clone()))
                        .map_err(|e| e.to_string())?;
                    let outputs = l
                        .session
                        .run(ort::inputs!["image" => tensor])
                        .map_err(|e| format!("Upscaling failed: {e}"))?;
                    let (_, data) = outputs["upscaled_image"]
                        .try_extract_tensor::<f32>()
                        .map_err(|e| e.to_string())?;
                    let oplane = (O * O) as usize;
                    for y in 0..S * X {
                        let oy = ty * X + y;
                        if oy >= h * X {
                            break;
                        }
                        for x in 0..S * X {
                            let ox = tx * X + x;
                            if ox >= w * X {
                                break;
                            }
                            let i = ((P * X + y) * O + (P * X + x)) as usize;
                            let px = [0, 1, 2].map(|c| (data[c * oplane + i] * 255.0).round().clamp(0.0, 255.0) as u8);
                            out.put_pixel(ox, oy, image::Rgb(px));
                        }
                    }
                    tx += S;
                }
                ty += S;
            }
            Ok(())
        })?;
        Ok(out)
    }
}

/// Exact Euclidean distance transform (Felzenszwalb & Huttenlocher), 1-D pass.
fn edt_1d(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let qf = q as f64;
        let mut s;
        loop {
            let p = v[k];
            let pf = p as f64;
            s = ((f[q] + qf * qf) - (f[p] + pf * pf)) / (2.0 * qf - 2.0 * pf);
            if s <= z[k] && k > 0 {
                k -= 1;
            } else {
                break;
            }
        }
        if s <= z[k] {
            // k == 0 and the new parabola dominates everywhere.
            v[0] = q;
            z[0] = f64::NEG_INFINITY;
            z[1] = f64::INFINITY;
            continue;
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let dq = q as f64 - v[k] as f64;
        d[q] = dq * dq + f[v[k]];
    }
}

pub fn outline_pad(thickness: u32) -> u32 {
    if thickness == 0 { 0 } else { thickness + 2 }
}

/// Draws a solid sticker-style outline of `thickness` px around the subject.
/// The canvas grows so the outline is never clipped.
pub fn add_outline(img: RgbaImage, thickness: u32, color: [u8; 3]) -> RgbaImage {
    if thickness == 0 {
        return img;
    }
    let pad = outline_pad(thickness);
    let (w0, h0) = img.dimensions();
    let (w, h) = (w0 + 2 * pad, h0 + 2 * pad);
    let mut canvas = RgbaImage::new(w, h);
    image::imageops::overlay(&mut canvas, &img, pad as i64, pad as i64);

    // Squared distance from every pixel to the nearest solid-ish subject pixel.
    const FAR: f64 = 1e12;
    let (wu, hu) = (w as usize, h as usize);
    let mut grid: Vec<f64> = canvas.pixels().map(|p| if p[3] >= 128 { 0.0 } else { FAR }).collect();
    let n = wu.max(hu);
    let (mut f, mut d, mut v, mut z) = (vec![0f64; n], vec![0f64; n], vec![0usize; n], vec![0f64; n + 1]);
    for x in 0..wu {
        for y in 0..hu {
            f[y] = grid[y * wu + x];
        }
        edt_1d(&f[..hu], &mut d[..hu], &mut v[..hu], &mut z[..hu + 1]);
        for y in 0..hu {
            grid[y * wu + x] = d[y];
        }
    }
    for y in 0..hu {
        f[..wu].copy_from_slice(&grid[y * wu..(y + 1) * wu]);
        edt_1d(&f[..wu], &mut d[..wu], &mut v[..wu], &mut z[..wu + 1]);
        grid[y * wu..(y + 1) * wu].copy_from_slice(&d[..wu]);
    }

    // Subject over a solid outline layer, with an anti-aliased outer edge.
    let t = thickness as f64;
    for (i, px) in canvas.pixels_mut().enumerate() {
        let dist = grid[i].sqrt();
        let ao = (t + 0.5 - dist).clamp(0.0, 1.0);
        if ao <= 0.0 {
            continue;
        }
        let as_ = px[3] as f64 / 255.0;
        let a = as_ + ao * (1.0 - as_);
        for c in 0..3 {
            let v = (px[c] as f64 * as_ + color[c] as f64 * ao * (1.0 - as_)) / a;
            px[c] = v.round().clamp(0.0, 255.0) as u8;
        }
        px[3] = (a * 255.0).round() as u8;
    }
    canvas
}

/// Box-blurs a single-channel f32 buffer in place (separable, via running sums).
fn box_blur(buf: &mut [f32], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0f32; buf.len()];
    for y in 0..h {
        let row = &buf[y * w..(y + 1) * w];
        let mut sum = 0f32;
        for x in 0..=r.min(w - 1) {
            sum += row[x];
        }
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            tmp[y * w + x] = sum / (hi - lo + 1) as f32;
            if x + r + 1 < w {
                sum += row[x + r + 1];
            }
            if x >= r {
                sum -= row[x - r];
            }
        }
    }
    for x in 0..w {
        let mut sum = 0f32;
        for y in 0..=r.min(h - 1) {
            sum += tmp[y * w + x];
        }
        for y in 0..h {
            let lo = y.saturating_sub(r);
            let hi = (y + r).min(h - 1);
            buf[y * w + x] = sum / (hi - lo + 1) as f32;
            if y + r + 1 < h {
                sum += tmp[(y + r + 1) * w + x];
            }
            if y >= r {
                sum -= tmp[(y - r) * w + x];
            }
        }
    }
}

/// Grayscale erosion: each pixel becomes the minimum alpha within `r` pixels, which pulls
/// every edge inward by `r` while keeping its soft profile. Separable (rows then columns),
/// and the photo border is ignored so products touching the frame aren't eaten into.
fn erode(alpha: &mut GrayImage, r: usize) {
    let (w, h) = (alpha.width() as usize, alpha.height() as usize);
    let src = alpha.as_raw().clone();
    let mut tmp = vec![0u8; w * h];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = *row[a..=b].iter().min().unwrap();
        }
    }
    let dst = alpha.as_mut();
    for x in 0..w {
        for y in 0..h {
            let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
            let mut m = 255u8;
            for yy in a..=b {
                m = m.min(tmp[yy * w + x]);
            }
            dst[y * w + x] = m;
        }
    }
}

/// Feathers the alpha with an approximate Gaussian (three box-blur passes).
fn feather(alpha: &mut GrayImage, r: usize) {
    let (w, h) = (alpha.width() as usize, alpha.height() as usize);
    let mut buf: Vec<f32> = alpha.as_raw().iter().map(|&v| v as f32).collect();
    let pass = ((r as f32) / 1.7).round().max(1.0) as usize;
    for _ in 0..3 {
        box_blur(&mut buf, w, h, pass);
    }
    for (d, v) in alpha.as_mut().iter_mut().zip(buf) {
        *d = v.round().clamp(0.0, 255.0) as u8;
    }
}

/// Removes the old background colour from semi-transparent edge pixels.
/// Estimates the local background colour B, then solves C = a*F + (1-a)*B for the true colour F.
fn decontaminate(img: &mut RgbaImage) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 2 || h < 2 {
        return;
    }
    let r = ((w.max(h) as f32) * 0.01).clamp(4.0, 40.0) as usize;
    let n = w * h;
    let mut bg_w = vec![0f32; n];
    let mut bg = [vec![0f32; n], vec![0f32; n], vec![0f32; n]];
    for (i, px) in img.pixels().enumerate() {
        let t = 1.0 - px[3] as f32 / 255.0; // how much of this pixel is background
        bg_w[i] = t;
        for c in 0..3 {
            bg[c][i] = px[c] as f32 * t;
        }
    }
    box_blur(&mut bg_w, w, h, r);
    for c in &mut bg {
        box_blur(c, w, h, r);
    }
    for (i, px) in img.pixels_mut().enumerate() {
        let a = px[3] as f32 / 255.0;
        if a <= 0.02 || a >= 0.98 || bg_w[i] < 1e-3 {
            continue;
        }
        for c in 0..3 {
            let b = bg[c][i] / bg_w[i];
            let f = (px[c] as f32 - (1.0 - a) * b) / a;
            px[c] = f.round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Crops away fully transparent borders, keeping `padding` pixels around the subject.
/// Also returns the top-left corner of the crop in the input image.
pub fn crop_to_subject(img: RgbaImage, padding: u32) -> (RgbaImage, u32, u32) {
    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for (x, y, px) in img.enumerate_pixels() {
        if px[3] > 8 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 > x1 || y0 > y1 {
        return (img, 0, 0); // nothing detected; leave as-is
    }
    let x0 = x0.saturating_sub(padding);
    let y0 = y0.saturating_sub(padding);
    let x1 = (x1 + padding).min(w - 1);
    let y1 = (y1 + padding).min(h - 1);
    (image::imageops::crop_imm(&img, x0, y0, x1 - x0 + 1, y1 - y0 + 1).to_image(), x0, y0)
}
