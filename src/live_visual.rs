//! Bounded, revision-cached spectrogram pixels for GPUI's single-image draw.
use crate::transport::{
    LIVE_SPECTROGRAM_BAND_COUNT, LIVE_SPECTROGRAM_MAX_HISTORY, LiveSpectrogramFrame,
};
use gpui::RenderImage;
use std::sync::{Arc, LazyLock};

// Modern terminal magnitude scale: black, charcoal grey, crimson, signal red, hot white.
// GPUI's sprite atlas consumes BGRA, rather than image's usual RGBA order.
static PALETTE: LazyLock<[[u8; 4]; 256]> = LazyLock::new(|| {
    const STOPS: [[u8; 3]; 9] = [
        [5, 5, 5],
        [24, 24, 24],
        [52, 52, 52],
        [92, 28, 30],
        [148, 24, 28],
        [208, 30, 34],
        [236, 66, 66],
        [248, 158, 158],
        [255, 255, 255],
    ];
    std::array::from_fn(|value| {
        let position = value as f32 / 255. * (STOPS.len() - 1) as f32;
        let lower = (position as usize).min(STOPS.len() - 2);
        let fraction = position - lower as f32;
        let rgb: [u8; 3] = std::array::from_fn(|channel| {
            (STOPS[lower][channel] as f32 * (1. - fraction)
                + STOPS[lower + 1][channel] as f32 * fraction)
                .round() as u8
        });
        [rgb[2], rgb[1], rgb[0], 255]
    })
});

pub fn magnitude_color(value: u8) -> [u8; 4] {
    PALETTE[value as usize]
}

#[derive(Default)]
pub struct SpectrogramImageCache {
    revision: Option<u64>,
    image: Option<Arc<RenderImage>>,
}

impl SpectrogramImageCache {
    /// Returns the current image and a retired texture to explicitly remove
    /// from GPUI's atlas. Resize, playhead ticks and pause reuse the same image.
    pub fn update(&mut self, frame: Option<&LiveSpectrogramFrame>) -> Option<Arc<RenderImage>> {
        let revision = frame.map(LiveSpectrogramFrame::gpu_revision);
        if revision == self.revision {
            return None;
        }
        self.revision = revision;
        let image = frame.map(|frame| {
            let pixels = spectrogram_pixels(frame);
            let buffer = image::RgbaImage::from_raw(
                LIVE_SPECTROGRAM_MAX_HISTORY as u32,
                LIVE_SPECTROGRAM_BAND_COUNT as u32,
                pixels,
            )
            .expect("validated live frames have fixed bounded image dimensions");
            Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
        });
        std::mem::replace(&mut self.image, image)
    }

    pub fn image(&self) -> Option<Arc<RenderImage>> {
        self.image.clone()
    }
}

fn spectrogram_pixels(frame: &LiveSpectrogramFrame) -> Vec<u8> {
    let width = LIVE_SPECTROGRAM_MAX_HISTORY;
    let height = LIVE_SPECTROGRAM_BAND_COUNT;
    let mut pixels = vec![0; width * height * 4];
    let start = width - frame.row_count;
    for y in 0..height {
        let band = height - 1 - y; // High frequency at the top.
        let row_pixels = &mut pixels[y * width * 4..(y + 1) * width * 4];
        for x in 0..width {
            let value = if x < start {
                0
            } else {
                frame.value(x - start, band)
            };
            row_pixels[x * 4..x * 4 + 4].copy_from_slice(&PALETTE[value as usize]);
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::LIVE_SPECTRUM_POINT_COUNT;

    fn frame(values: Vec<u8>, rows: usize) -> LiveSpectrogramFrame {
        LiveSpectrogramFrame::from_values(
            1,
            1,
            1,
            48_000,
            rows,
            values.into(),
            vec![0; LIVE_SPECTRUM_POINT_COUNT].into(),
        )
        .unwrap()
    }

    #[test]
    fn heatmap_places_newest_time_at_right_and_low_frequencies_at_bottom() {
        let mut values = vec![0; LIVE_SPECTROGRAM_BAND_COUNT * 2];
        values[0] = 255; // Earlier bass energy.
        values[2 * LIVE_SPECTROGRAM_BAND_COUNT - 1] = 128; // Latest treble energy.
        let pixels = spectrogram_pixels(&frame(values, 2));
        let width = LIVE_SPECTROGRAM_MAX_HISTORY;
        let pixel = |x, y| &pixels[(y * width + x) * 4..(y * width + x + 1) * 4];
        assert_eq!(pixel(width - 1, 0), magnitude_color(128));
        assert_eq!(
            pixel(width - 2, LIVE_SPECTROGRAM_BAND_COUNT - 1),
            magnitude_color(255)
        );
        assert_eq!(pixel(0, 0), magnitude_color(0));
        assert_ne!(magnitude_color(32), magnitude_color(192));
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 255)
        );
    }

    #[test]
    fn unchanged_frame_reuses_image_and_source_change_retires_old_texture() {
        let first = frame(vec![64; LIVE_SPECTROGRAM_BAND_COUNT], 1);
        let mut cache = SpectrogramImageCache::default();
        assert!(cache.update(Some(&first)).is_none());
        let original = cache.image().unwrap();
        assert!(cache.update(Some(&first)).is_none());
        assert!(Arc::ptr_eq(&original, &cache.image().unwrap()));
        // Even identical local revision counters from a new session must invalidate.
        let next = frame(vec![192; LIVE_SPECTROGRAM_BAND_COUNT], 1);
        assert!(Arc::ptr_eq(&original, &cache.update(Some(&next)).unwrap()));
        assert!(!Arc::ptr_eq(&original, &cache.image().unwrap()));
        assert!(cache.update(None).is_some());
        assert!(cache.image().is_none());
        assert!(cache.update(None).is_none());
    }

    #[test]
    fn terminal_palette_spans_black_neutral_grey_signal_red_and_white() {
        let floor = magnitude_color(0);
        assert_eq!([floor[2], floor[1], floor[0], floor[3]], [5, 5, 5, 255]);
        let grey = magnitude_color(64);
        assert_eq!(grey[0], grey[1]);
        assert_eq!(grey[1], grey[2]);
        let red = magnitude_color(160);
        assert!(red[2] > 180 && red[1] < 50 && red[0] < 50);
        let peak = magnitude_color(255);
        assert_eq!([peak[2], peak[1], peak[0], peak[3]], [255, 255, 255, 255]);
    }
}
