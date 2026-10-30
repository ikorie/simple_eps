#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use eframe::egui;
use image::DynamicImage;

const PREVIEW_DPI: u32 = 144;
const JPEG_QUALITY: u8 = 95;

fn main() -> eframe::Result<()> {
    let initial_path = env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 700.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Simple EPS",
        options,
        Box::new(move |_| Ok(Box::new(EpsApp::new(initial_path)))),
    )
}

struct EpsApp {
    source_path: Option<PathBuf>,
    image: Option<DynamicImage>,
    texture: Option<egui::TextureHandle>,
    zoom: f32,
    pan: egui::Vec2,
    drag_delta: egui::Vec2,
    fit_to_canvas: bool,
    status: String,
    initial_path: Option<PathBuf>,
}

impl EpsApp {
    fn new(initial_path: Option<PathBuf>) -> Self {
        Self {
            source_path: None,
            image: None,
            texture: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            drag_delta: egui::Vec2::ZERO,
            fit_to_canvas: false,
            status: String::new(),
            initial_path,
        }
    }

    fn open_eps(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("EPS files", &["eps", "EPS"])
            .pick_file()
        else {
            return;
        };

        self.open_eps_path(ctx, path);
    }

    fn open_eps_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        match render_eps(&path) {
            Ok(image) => {
                let size = [image.width() as usize, image.height() as usize];
                let pixels = image.to_rgba8();
                let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                self.texture = Some(ctx.load_texture(
                    "eps-preview",
                    color_image,
                    egui::TextureOptions::LINEAR,
                ));
                self.source_path = Some(path.clone());
                self.image = Some(image);
                self.zoom = 1.0;
                self.pan = egui::Vec2::ZERO;
                self.drag_delta = egui::Vec2::ZERO;
                self.fit_to_canvas = true;
                self.status = format!("Opened {}", path.display());
            }
            Err(error) => self.status = format!("Could not open {}: {error}", path.display()),
        }
    }

    fn save_jpeg(&mut self) {
        let Some(image) = &self.image else {
            return;
        };

        let default_name = self
            .source_path
            .as_deref()
            .and_then(Path::file_stem)
            .map(|stem| format!("{}.jpg", stem.to_string_lossy()))
            .unwrap_or_else(|| "image.jpg".to_owned());

        let Some(mut path) = rfd::FileDialog::new()
            .add_filter("JPEG image", &["jpg", "jpeg"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };

        if path.extension().is_none() {
            path.set_extension("jpg");
        }

        match save_as_jpeg(image, &path) {
            Ok(()) => self.status = format!("Saved {}", path.display()),
            Err(error) => self.status = format!("Could not save {}: {error}", path.display()),
        }
    }
}

impl eframe::App for EpsApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if let Some(path) = self.initial_path.take() {
            self.open_eps_path(ctx, path);
        }

        let dropped_files = ctx.input(|input| input.raw.dropped_files.clone());
        if let Some(path) = dropped_files.into_iter().find_map(|file| file.path) {
            if is_eps_file(&path) {
                self.open_eps_path(ctx, path);
            } else {
                self.status = format!("{} is not an EPS file.", path.display());
            }
        }

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open EPS...").clicked() {
                    self.open_eps(ctx);
                }
                if ui
                    .add_enabled(self.image.is_some(), egui::Button::new("Save as JPEG..."))
                    .clicked()
                {
                    self.save_jpeg();
                }

                if self.image.is_some() {
                    ui.separator();
                    ui.label("Mouse wheel: zoom / Drag: pan");
                }
            });
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.label(if self.status.is_empty() {
                "Open an EPS file to preview it and export a JPEG."
            } else {
                &self.status
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let Some((texture_id, texture_size)) = self
                .texture
                .as_ref()
                .map(|texture| (texture.id(), texture.size_vec2()))
            else {
                ui.centered_and_justified(|ui| {
                    ui.heading("Open or drop an EPS file");
                });
                return;
            };

            let canvas = ui.max_rect();
            let response = ui.interact(canvas, ui.id().with("image-canvas"), egui::Sense::drag());

            if self.fit_to_canvas {
                self.zoom = (canvas.width() / texture_size.x)
                    .min(canvas.height() / texture_size.y)
                    .min(1.0)
                    .clamp(0.1, 4.0);
                self.pan = egui::Vec2::ZERO;
                self.fit_to_canvas = false;
            }

            if response.drag_started() {
                self.drag_delta = egui::Vec2::ZERO;
            }
            if response.dragged() {
                let new_drag_delta = response.drag_delta();
                self.pan += new_drag_delta - self.drag_delta;
                self.drag_delta = new_drag_delta;
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if response.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::Grab);
            }

            if response.hovered() {
                let scroll_delta = ctx.input(|input| input.raw_scroll_delta.y);
                if scroll_delta != 0.0 {
                    let old_zoom = self.zoom;
                    self.zoom = (self.zoom * (scroll_delta * 0.002).exp()).clamp(0.1, 4.0);

                    if self.zoom != old_zoom {
                        let pointer = ctx
                            .input(|input| input.pointer.hover_pos())
                            .unwrap_or(canvas.center());
                        let pointer_offset = pointer - (canvas.min + self.pan);
                        self.pan = pointer - canvas.min - pointer_offset * (self.zoom / old_zoom);
                    }
                }
            }

            let image_size = texture_size * self.zoom;
            let image_rect = egui::Rect::from_min_size(canvas.min + self.pan, image_size);
            ui.painter_at(canvas).image(
                texture_id,
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        });
    }
}

fn is_eps_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("eps"))
}

fn render_eps(input: &Path) -> Result<DynamicImage, String> {
    let output = temporary_png_path()?;
    let ghostscript = find_ghostscript()?;
    let result = Command::new(ghostscript)
        .args([
            "-dSAFER",
            "-dBATCH",
            "-dNOPAUSE",
            "-dEPSCrop",
            "-sDEVICE=pngalpha",
            &format!("-r{PREVIEW_DPI}"),
            &format!("-sOutputFile={}", output.display()),
        ])
        .arg(input)
        .output()
        .map_err(|error| format!("failed to start Ghostscript: {error}"))?;

    if !result.status.success() {
        let _ = fs::remove_file(&output);
        return Err(String::from_utf8_lossy(&result.stderr).trim().to_owned());
    }

    let image = image::open(&output).map_err(|error| format!("failed to read preview: {error}"));
    let _ = fs::remove_file(output);
    image
}

fn save_as_jpeg(image: &DynamicImage, path: &Path) -> Result<(), String> {
    let file = fs::File::create(path).map_err(|error| error.to_string())?;
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, JPEG_QUALITY);
    encoder
        .encode_image(image)
        .map_err(|error| error.to_string())
}

fn find_ghostscript() -> Result<PathBuf, String> {
    if let Some(executable) = bundled_ghostscript() {
        return Ok(executable);
    }

    for executable in ["gswin64c.exe", "gswin32c.exe", "gs"] {
        if let Some(path) = find_in_path(executable) {
            return Ok(path);
        }
    }

    for program_files in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = env::var_os(program_files) {
            let directory = Path::new(&root).join("gs");
            if let Ok(versions) = fs::read_dir(directory) {
                for version in versions.flatten() {
                    let executable = version.path().join("bin").join("gswin64c.exe");
                    if executable.is_file() {
                        return Ok(executable);
                    }
                }
            }
        }
    }

    Err("Ghostscript was not found. Install Ghostscript and add it to PATH.".to_owned())
}

fn bundled_ghostscript() -> Option<PathBuf> {
    let executable = env::current_exe()
        .ok()?
        .parent()?
        .join("runtime")
        .join("ghostscript")
        .join("bin")
        .join("gswin64c.exe");
    executable.is_file().then_some(executable)
}

fn find_in_path(executable: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|directory| directory.join(executable))
            .find(|candidate| candidate.is_file())
    })
}

fn temporary_png_path() -> Result<PathBuf, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    Ok(env::temp_dir().join(format!("simple-eps-{timestamp}.png")))
}
