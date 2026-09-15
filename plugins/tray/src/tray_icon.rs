use tauri::{Result, image::Image};

pub enum TrayIconState {
    Default,
    Degraded,
    UpdateAvailable,
}

pub const RECORDING_FRAMES: &[&[u8]] = &[
    include_bytes!("../icons/tray_recording_0.png"),
    include_bytes!("../icons/tray_recording_1.png"),
    include_bytes!("../icons/tray_recording_2.png"),
    include_bytes!("../icons/tray_recording_3.png"),
];

impl TrayIconState {
    pub fn to_image(&self) -> Result<Image<'static>> {
        #[cfg(target_os = "windows")]
        return windows_image(match self {
            Self::Default => None,
            Self::Degraded => Some(([180, 83, 9], '!')),
            Self::UpdateAvailable => Some(([29, 78, 216], '^')),
        });
        #[cfg(not(target_os = "windows"))]
        match self {
            TrayIconState::Default => {
                Image::from_bytes(include_bytes!("../icons/tray_default.png"))
            }
            TrayIconState::Degraded => {
                Image::from_bytes(include_bytes!("../icons/tray_degraded.png"))
            }
            TrayIconState::UpdateAvailable => {
                Image::from_bytes(include_bytes!("../icons/tray_update.png"))
            }
        }
    }
}

pub fn recording_frame(index: usize) -> Result<Image<'static>> {
    #[cfg(target_os = "windows")]
    {
        let red = [185, 220, 239, 220][index % 4];
        windows_image(Some(([red, 38, 38], ' ')))
    }
    #[cfg(not(target_os = "windows"))]
    Image::from_bytes(RECORDING_FRAMES[index])
}

#[cfg(target_os = "windows")]
fn windows_image(badge: Option<([u8; 3], char)>) -> Result<Image<'static>> {
    // macOS template images are monochrome masks. A full-color base stays
    // visible on both light and dark Windows taskbars without theme polling.
    let base = Image::from_bytes(include_bytes!(
        "../../../apps/desktop/src-tauri/icons/stable/32x32.png"
    ))?;
    let Some((color, glyph)) = badge else {
        return Ok(base);
    };
    let mut rgba = base.rgba().to_vec();
    for y in 16..32usize {
        for x in 16..32usize {
            let distance = (x as f32 - 24.0).hypot(y as f32 - 24.0);
            if distance <= 7.0 {
                let pixel = &mut rgba[(y * 32 + x) * 4..(y * 32 + x + 1) * 4];
                let background = if distance >= 6.0 {
                    [255, 255, 255, 255]
                } else {
                    [color[0], color[1], color[2], 255]
                };
                pixel.copy_from_slice(&background);
                let mark = match glyph {
                    '!' => x == 24 && ((20..=24).contains(&y) || y == 27),
                    '^' => {
                        (x == 24 && (20..=27).contains(&y))
                            || ((21..=23).contains(&y) && x.abs_diff(24) == y - 20)
                    }
                    _ => false,
                };
                if mark {
                    pixel.copy_from_slice(&[255; 4]);
                }
            }
        }
    }
    Ok(Image::new_owned(rgba, 32, 32))
}
