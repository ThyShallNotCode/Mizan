//! Renders the design gallery (`Gallery` = Review Queue screen, `Sheet` = component
//! sheet) with Slint's software renderer into PNG files, light and dark.
//! Usage: omr-design-gallery <out-prefix> [width] [height]
//!
//! Time is frozen (duration_since_start = 0), so the gallery sets
//! `Theme.reduce-motion = true`; otherwise animated colours would be captured
//! mid-transition. Headless screenshot tests in the shell need the same switch.
//!
//! No `#![forbid(unsafe_code)]` here: with this feature set, Slint 1.18 generated code
//! expands a vtable macro carrying `#[allow(unsafe_code)]`, which `forbid` rejects (E0453).
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};
use std::rc::Rc;

slint::include_modules!();

struct HeadlessPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> core::time::Duration {
        core::time::Duration::from_secs(0)
    }
}

fn render(window: &MinimalSoftwareWindow, w: u32, h: u32, path: &str) {
    let mut buffer = vec![Rgb8Pixel { r: 0, g: 0, b: 0 }; (w * h) as usize];
    window.request_redraw();
    window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, w as usize);
    });
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("png header");
    let bytes: Vec<u8> = buffer.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    writer.write_image_data(&bytes).expect("png data");
    println!("wrote {path}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let prefix = args.get(1).cloned().unwrap_or_else(|| "gallery".into());
    let w: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1440);
    let h: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(900);
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(HeadlessPlatform {
        window: window.clone(),
    }))
    .expect("set platform");
    window.set_size(PhysicalSize::new(w, h));
    {
        let ui = Gallery::new().expect("create gallery");
        ui.show().expect("show");
        for dark in [false, true] {
            ui.set_dark(dark);
            slint::platform::update_timers_and_animations();
            render(
                &window,
                w,
                h,
                &format!(
                    "{prefix}-review-{}.png",
                    if dark { "dark" } else { "light" }
                ),
            );
        }
        ui.hide().expect("hide");
    }
    {
        let ui = Sheet::new().expect("create sheet");
        ui.show().expect("show");
        for dark in [false, true] {
            ui.set_dark(dark);
            slint::platform::update_timers_and_animations();
            render(
                &window,
                w,
                h,
                &format!(
                    "{prefix}-components-{}.png",
                    if dark { "dark" } else { "light" }
                ),
            );
        }
    }
}
