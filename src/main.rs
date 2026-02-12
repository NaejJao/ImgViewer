#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use image::GenericImageView;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

struct LoadedImage {
    path: PathBuf,
    tiles: Vec<ImageTile>,
    full_size: egui::Vec2,
}

struct ImageTile {
    texture: egui::TextureHandle,
    rect: egui::Rect,
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: viewer <path_to_image>");
        std::process::exit(1);
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_fullscreen(true)
            .with_active(true), // Explicitly request active state on launch
        ..Default::default()
    };

    eframe::run_native(
        "Ultimate Lean Viewer",
        options,
        Box::new(|cc| Ok(Box::new(LeanViewer::new(cc, PathBuf::from(&args[1]))))),
    )
}

struct LeanViewer {
    tiles: Vec<ImageTile>,
    full_size: egui::Vec2,
    offset: egui::Vec2,
    zoom: f32,
    first_frame: bool,
    current_path: PathBuf,
    album: Vec<PathBuf>,
    rx: Receiver<LoadedImage>,
    tx: Sender<LoadedImage>,
}

impl LeanViewer {
    pub fn new(cc: &eframe::CreationContext<'_>, path: PathBuf) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let (tiles, full_size, album) = Self::load_assets(&cc.egui_ctx, &path);
        Self {
            tiles,
            full_size,
            offset: egui::Vec2::ZERO,
            zoom: 1.0,
            first_frame: true,
            current_path: path,
            album,
            rx,
            tx,
        }
    }

    fn load_assets(
        ctx: &egui::Context,
        path: &PathBuf,
    ) -> (Vec<ImageTile>, egui::Vec2, Vec<PathBuf>) {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        let img = if ext == "heic" || ext == "heif" {
            Self::decode_heic(path).expect("HEIC decoding failed")
        } else {
            image::open(path).expect("Failed to open image")
        };

        let (width, height) = img.dimensions();
        let tile_limit = 2048;
        let mut tiles = Vec::new();

        for y in (0..height).step_by(tile_limit) {
            for x in (0..width).step_by(tile_limit) {
                let tw = (tile_limit as u32).min(width - x);
                let th = (tile_limit as u32).min(height - y);
                let tile_view = img.view(x, y, tw, th).to_image();
                let color_image = egui::ColorImage::from_rgba_unmultiplied(
                    [tw as usize, th as usize],
                    &tile_view,
                );

                let tex_name = format!("{}_{}_{}", path.display(), x, y);
                let texture = ctx.load_texture(tex_name, color_image, egui::TextureOptions::LINEAR);

                tiles.push(ImageTile {
                    texture,
                    rect: egui::Rect::from_min_size(
                        egui::pos2(x as f32, y as f32),
                        egui::vec2(tw as f32, th as f32),
                    ),
                });
            }
        }

        let mut album = Vec::new();
        if let Some(parent) = path.parent() {
            if let Ok(entries) = std::fs::read_dir(parent) {
                album = entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| {
                        let e = p
                            .extension()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_lowercase();
                        matches!(
                            e.as_str(),
                            "jpg"
                                | "jpeg"
                                | "png"
                                | "webp"
                                | "bmp"
                                | "gif"
                                | "heic"
                                | "heif"
                                | "tiff"
                                | "tga"
                        )
                    })
                    .collect();
                album.sort();
            }
        }
        (tiles, egui::vec2(width as f32, height as f32), album)
    }

    fn decode_heic(path: &PathBuf) -> Result<image::DynamicImage, Box<dyn std::error::Error>> {
        let context = libheif_rs::HeifContext::read_from_file(path.to_str().unwrap())?;
        let handle = context.primary_image_handle()?;
        let libheif = libheif_rs::LibHeif::new();

        let image = libheif.decode(
            &handle,
            libheif_rs::ColorSpace::Rgb(libheif_rs::RgbChroma::Rgba),
            None,
        )?;

        let width = image.width();
        let height = image.height();
        let interleaved = image.planes().interleaved.ok_or("No interleaved plane")?;

        let mut rgba_data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let start = (y as usize) * interleaved.stride;
            let end = start + (width as usize) * 4;
            rgba_data.extend_from_slice(&interleaved.data[start..end]);
        }

        let buffer = image::RgbaImage::from_raw(width, height, rgba_data).ok_or("Buffer fail")?;
        Ok(image::DynamicImage::ImageRgba8(buffer))
    }

    fn preload(&self, ctx: egui::Context, delta: i32) {
        if let Some(pos) = self.album.iter().position(|p| p == &self.current_path) {
            let new_index = (pos as i32 + delta).rem_euclid(self.album.len() as i32) as usize;
            let path = self.album[new_index].clone();
            let tx = self.tx.clone();

            std::thread::spawn(move || {
                let (tiles, full_size, _) = Self::load_assets(&ctx, &path);
                let _ = tx.send(LoadedImage {
                    path,
                    tiles,
                    full_size,
                });
                ctx.request_repaint();
            });
        }
    }
}

impl eframe::App for LeanViewer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.first_frame {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        if let Ok(loaded) = self.rx.try_recv() {
            self.tiles = loaded.tiles;
            self.full_size = loaded.full_size;
            self.current_path = loaded.path;
            self.offset = egui::Vec2::ZERO;
            self.first_frame = true;
        }

        ctx.input(|i| {
            if i.key_pressed(egui::Key::ArrowRight) {
                self.preload(ctx.clone(), 1);
            }
            if i.key_pressed(egui::Key::ArrowLeft) {
                self.preload(ctx.clone(), -1);
            }
            if i.key_pressed(egui::Key::Escape) {
                std::process::exit(0);
            }
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                if self.first_frame {
                    ui.memory_mut(|mem| mem.request_focus(egui::Id::new("main_view")));
                }
                let screen_rect = ui.max_rect();

                let fit_zoom = (screen_rect.width() / self.full_size.x)
                    .min(screen_rect.height() / self.full_size.y);

                if self.first_frame && screen_rect.width() > 1.0 {
                    self.zoom = fit_zoom;
                    self.first_frame = false;
                }

                ui.input(|i| {
                    if i.smooth_scroll_delta.y != 0.0 {
                        let zoom_factor = (i.smooth_scroll_delta.y * 0.005).exp();
                        self.zoom *= zoom_factor;
                        if let Some(mouse_pos) = i.pointer.hover_pos() {
                            let center = screen_rect.center() + self.offset;
                            self.offset -= (mouse_pos - center) * (zoom_factor - 1.0);
                        }
                    }
                    if i.pointer.any_down() {
                        self.offset += i.pointer.delta();
                    }

                    // Double Click reset OR press 'F'
                    if i.pointer
                        .button_double_clicked(egui::PointerButton::Primary)
                        || i.key_pressed(egui::Key::F)
                    {
                        self.zoom = fit_zoom;
                        self.offset = egui::Vec2::ZERO;
                    }
                    // Press '1' for 100% scale
                    if i.key_pressed(egui::Key::Num1) {
                        self.zoom = 1.0;
                        self.offset = egui::Vec2::ZERO;
                    }
                });

                // Render Tiles
                let center = screen_rect.center() + self.offset;
                for tile in &self.tiles {
                    let tile_size = tile.rect.size() * self.zoom;
                    let rel_pos = (tile.rect.min.to_vec2() - self.full_size / 2.0) * self.zoom;
                    let rect = egui::Rect::from_min_size(center + rel_pos, tile_size);
                    ui.painter().image(
                        tile.texture.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }

                // UI Overlay
                let filename = self
                    .current_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                let info_text = format!(
                    "{} • {:.0}x{:.0} • {:.0}%",
                    filename,
                    self.full_size.x,
                    self.full_size.y,
                    self.zoom * 100.0
                );
                ui.painter().text(
                    screen_rect.left_bottom() + egui::vec2(10.0, -10.0),
                    egui::Align2::LEFT_BOTTOM,
                    info_text,
                    egui::FontId::proportional(14.0),
                    egui::Color32::from_white_alpha(120),
                );
            });
    }
}
