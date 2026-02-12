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
            .with_active(true),
        ..Default::default()
    };

    eframe::run_native(
        "ImgViewer",
        options,
        Box::new(|cc| Ok(Box::new(LeanViewer::new(cc, PathBuf::from(&args[1]))))),
    )
}

struct LeanViewer {
    tiles: Vec<ImageTile>,
    full_size: egui::Vec2,
    offset: egui::Vec2,
    zoom: f32,
    rotation_steps: i32,
    first_frame: bool,
    current_path: PathBuf,
    album: Vec<PathBuf>,
    rx: Receiver<LoadedImage>,
    tx: Sender<LoadedImage>,
    show_about: bool,
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
            rotation_steps: 0,
            first_frame: true,
            current_path: path,
            album,
            rx,
            tx,
            show_about: false,
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
        if let Ok(loaded) = self.rx.try_recv() {
            self.tiles = loaded.tiles;
            self.full_size = loaded.full_size;
            self.current_path = loaded.path;
            self.offset = egui::Vec2::ZERO;
            self.rotation_steps = 0;
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
            if i.key_pressed(egui::Key::R) {
                self.rotation_steps = (self.rotation_steps + 1) % 4;
            }
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                let display_rect = ui.max_rect();
                let is_sideways = self.rotation_steps % 2 != 0;
                let effective_size = if is_sideways { egui::vec2(self.full_size.y, self.full_size.x) } else { self.full_size };

                let fit_zoom = (display_rect.width() / effective_size.x).min(display_rect.height() / effective_size.y);
                if self.first_frame && display_rect.width() > 1.0 {
                    self.zoom = fit_zoom;
                    self.first_frame = false;
                }

                let (_rect, response) = ui.allocate_at_least(ui.available_size(), egui::Sense::click_and_drag());
                if response.double_clicked() {
                    if (self.zoom - fit_zoom).abs() < 0.01 { self.zoom = 1.0; } else { self.zoom = fit_zoom; }
                    self.offset = egui::Vec2::ZERO;
                }

                ui.input(|i| {
                    if i.key_pressed(egui::Key::F) {
                        if (self.zoom - fit_zoom).abs() < 0.01 { self.zoom = 1.0; } else { self.zoom = fit_zoom; }
                        self.offset = egui::Vec2::ZERO;
                    }
                    if i.smooth_scroll_delta.y != 0.0 {
                        let zoom_factor = (i.smooth_scroll_delta.y * 0.005).exp();
                        self.zoom *= zoom_factor;
                        if let Some(mouse_pos) = i.pointer.hover_pos() {
                            let center = display_rect.center() + self.offset;
                            self.offset -= (mouse_pos - center) * (zoom_factor - 1.0);
                        }
                    }
                    if i.pointer.any_down() { self.offset += i.pointer.delta(); }
                });

                response.context_menu(|ui| {
                    if ui.button("About").clicked() { self.show_about = true; ui.close_kind(egui::UiKind::Menu); }
                    ui.separator();
                    if ui.button("Exit").clicked() { std::process::exit(0); }
                });

                let center = display_rect.center() + self.offset;
                let rotation_angle = self.rotation_steps as f32 * std::f32::consts::FRAC_PI_2;
                let rot = egui::emath::Rot2::from_angle(rotation_angle);

                for tile in &self.tiles {
                    let tile_size = tile.rect.size() * self.zoom;

                    // Convert relative position to Vec2 before rotation
                    let tile_rel_to_center = (tile.rect.center() - (self.full_size / 2.0)).to_vec2();
                    let rotated_pos = rot * tile_rel_to_center;

                    let rect = egui::Rect::from_center_size(center + rotated_pos * self.zoom, tile_size);

                    let mut mesh = egui::Mesh::with_texture(tile.texture.id());
                    mesh.add_rect_with_uv(
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                    mesh.rotate(rot, rect.center());
                    ui.painter().add(mesh);
                }

                if self.show_about {
                    egui::Window::new("About")
                        .collapsible(false).resizable(false)
                        .pivot(egui::Align2::CENTER_CENTER)
                        .default_pos(display_rect.center())
                        .show(ctx, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.heading("ImgViewer");
                                ui.label("v0.1.0");
                                ui.separator();
                                ui.label("A lean, mean tiled image viewer written in Rust.\n\nDeveloper: Jean Schifflers.");
                                ui.separator();
                                ui.vertical(|ui| {
                                    ui.label(egui::RichText::new("Keyboard shortcuts:").strong());
                                    ui.label("• F: Toggle Zoom ( fit / 100% )");
                                    ui.label("• R: Rotate 90° clockwise");
                                    ui.label("• ESC: Exit");
                                    ui.label("• Arrows: Navigate album");
                                });
                                ui.add_space(10.0);
                                if ui.button("Close").clicked() { self.show_about = false; }
                            });
                        });
                }
            });
    }
}
