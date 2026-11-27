//! macOS GPU conversion for the live spectrogram.
//!
//! The worker owns Metal and CoreVideo state. Completed IOSurface-backed
//! buffers cross to GPUI only after the Metal command has finished, and are
//! immutable from that point on. The pool is capped so scenes retained by
//! GPUI can apply backpressure without blocking the UI or analyzer.

use core_foundation::{
    base::{CFAllocatorRef, CFType, TCFType, kCFAllocatorDefault},
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFDictionaryRef},
    number::CFNumber,
    string::CFString,
};
use core_video::{
    metal_texture::{CVMetalTextureGetTexture, CVMetalTextureKeys},
    metal_texture_cache::{CVMetalTextureCache, CVMetalTextureCacheRef},
    pixel_buffer::{
        CVPixelBuffer, CVPixelBufferKeys, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    },
    pixel_buffer_pool::{CVPixelBufferPool, CVPixelBufferPoolKeys},
    r#return::{CVReturn, kCVReturnSuccess, kCVReturnWouldExceedAllocationThreshold},
};
use metal::{
    CommandQueue, ComputePipelineState, Device, MTLResourceOptions, MTLSize, MTLTextureUsage,
    TextureRef,
    foreign_types::{ForeignType, ForeignTypeRef},
};
use objc::rc::autoreleasepool;
use std::{
    ffi::c_void,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

// core-video 0.4.3 takes an owned Metal Device wrapper by value in this C ABI
// declaration, which forgets its Rust destructor after returning. Mirror the
// native borrowed id<MTLDevice> pointer ABI to keep ownership balanced.
unsafe extern "C" {
    #[link_name = "CVMetalTextureCacheCreate"]
    fn create_metal_texture_cache(
        allocator: CFAllocatorRef,
        cache_attributes: CFDictionaryRef,
        metal_device: *mut c_void,
        texture_attributes: CFDictionaryRef,
        cache_out: *mut CVMetalTextureCacheRef,
    ) -> CVReturn;
}

const INPUT_WIDTH: usize = 240;
const INPUT_HEIGHT: usize = 128;
const OUTPUT_WIDTH: usize = INPUT_WIDTH * 2;
const OUTPUT_HEIGHT: usize = INPUT_HEIGHT * 2;
const POOL_LIMIT: i32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameKey {
    source: usize,
    generation: u64,
    epoch: u64,
    gpu_revision: u64,
}

impl FrameKey {
    fn new(source: usize, frame: &crate::transport::LiveSpectrogramFrame) -> Self {
        Self {
            source,
            generation: frame.generation,
            epoch: frame.epoch,
            gpu_revision: frame.gpu_revision(),
        }
    }

    fn same_source_epoch(self, other: Self) -> bool {
        self.source == other.source
            && self.generation == other.generation
            && self.epoch == other.epoch
    }
}

struct Request {
    key: FrameKey,
    rows: usize,
    values: Arc<[u8]>,
}

struct PublishedSurface {
    key: FrameKey,
    buffer: CVPixelBuffer,
}

// SAFETY: CVPixelBuffer itself is not Send/Sync. This particular instance is
// retained by CoreVideo, written only on the worker before GPU completion,
// and thereafter read-only. GPUI's scene retains another CF reference for the
// duration of each import/draw. No code returns this pool allocation to the
// worker or mutates it after publishing.
unsafe impl Send for PublishedSurface {}
unsafe impl Sync for PublishedSurface {}

#[derive(Default)]
struct Shared {
    requested: Option<Request>,
    wanted: Option<FrameKey>,
    ready: Option<Arc<PublishedSurface>>,
    failed: bool,
    stopped: bool,
}

pub struct GpuSpectrogramRenderer {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    submitted: Option<FrameKey>,
    surface: Option<Arc<PublishedSurface>>,
    failed: bool,
}

impl Default for GpuSpectrogramRenderer {
    fn default() -> Self {
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let worker_shared = Arc::clone(&shared);
        if thread::Builder::new()
            .name("cadence-spectrogram-metal".into())
            .spawn(move || worker_main(worker_shared))
            .is_err()
            && let Ok(mut state) = shared.0.lock()
        {
            state.failed = true;
        }
        Self {
            shared,
            submitted: None,
            surface: None,
            failed: false,
        }
    }
}

impl GpuSpectrogramRenderer {
    /// Queues at most the latest frame and adopts a completed surface without
    /// waiting. Call from the normal GPUI frame path, including while paused.
    pub fn poll(
        &mut self,
        source: usize,
        frame: Option<&crate::transport::LiveSpectrogramFrame>,
    ) -> bool {
        let previous_surface = self.surface.as_ref().map(|surface| surface.key);
        let previous_failed = self.failed;
        if let Ok(state) = self.shared.0.lock() {
            self.failed |= state.failed;
        }
        if self.failed {
            self.surface = None;
            return previous_surface.is_some() || !previous_failed;
        }
        let key = frame.map(|frame| FrameKey::new(source, frame));
        if key != self.submitted {
            let same_epoch = matches!((key, self.submitted), (Some(next), Some(previous)) if next.same_source_epoch(previous));
            self.submitted = key;
            if !same_epoch {
                self.surface = None;
            }
            if let Ok(mut state) = self.shared.0.lock() {
                state.wanted = key;
                if !same_epoch {
                    state.ready = None;
                }
                state.requested = frame.map(|frame| Request {
                    key: key.expect("a frame always has a key"),
                    rows: frame.row_count,
                    values: Arc::clone(frame.packed_values()),
                });
                self.shared.1.notify_one();
            }
        }

        if let Ok(mut state) = self.shared.0.lock() {
            self.failed |= state.failed;
            if let Some(ready) = state.ready.take()
                && self
                    .submitted
                    .is_some_and(|wanted| ready.key.same_source_epoch(wanted))
            {
                self.surface = Some(ready);
            }
        }
        if self.failed {
            self.surface = None;
        }
        previous_surface != self.surface.as_ref().map(|surface| surface.key)
            || previous_failed != self.failed
    }

    pub fn is_failed(&self) -> bool {
        self.failed
    }

    pub fn surface(&self) -> Option<CVPixelBuffer> {
        self.surface.as_ref().map(|surface| surface.buffer.clone())
    }
}

impl Drop for GpuSpectrogramRenderer {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.0.lock() {
            state.stopped = true;
            state.requested = None;
            self.shared.1.notify_one();
        }
        // The detached worker observes stopped between frames. Drop never
        // waits for a GPU command or joins the worker on GPUI's UI thread.
    }
}

fn worker_main(shared: Arc<(Mutex<Shared>, Condvar)>) {
    let engine = match autoreleasepool(MetalEngine::new) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("Cadence spectrogram GPU unavailable: {error}");
            if let Ok(mut state) = shared.0.lock() {
                state.failed = true;
            }
            return;
        }
    };

    loop {
        let request = {
            let mut state = match shared.0.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            while state.requested.is_none() && !state.stopped {
                state = match shared.1.wait(state) {
                    Ok(state) => state,
                    Err(_) => return,
                };
            }
            if state.stopped {
                return;
            }
            state.requested.take().expect("checked above")
        };

        let result = autoreleasepool(|| engine.render(&request));
        match result {
            Ok(Some(buffer)) => {
                let mut state = match shared.0.lock() {
                    Ok(state) => state,
                    Err(_) => return,
                };
                if state
                    .wanted
                    .is_some_and(|wanted| request.key.same_source_epoch(wanted))
                    && !state.stopped
                {
                    state.ready = Some(Arc::new(PublishedSurface {
                        key: request.key,
                        buffer,
                    }));
                }
            }
            Ok(None) => {
                let mut state = match shared.0.lock() {
                    Ok(state) => state,
                    Err(_) => return,
                };
                if state.wanted == Some(request.key) && state.requested.is_none() && !state.stopped
                {
                    state.requested = Some(request);
                }
                drop(state);
                // Let GPUI release any finished scene references before
                // retrying pool allocation. This worker-only delay preserves
                // a paused final frame without a busy loop.
                thread::sleep(Duration::from_millis(40));
            }
            Err(error) => {
                eprintln!("Cadence spectrogram GPU render failed: {error}");
                if let Ok(mut state) = shared.0.lock() {
                    state.failed = true;
                    state.ready = None;
                }
            }
        }
    }
}

struct MetalEngine {
    device: Device,
    queue: CommandQueue,
    pipeline: ComputePipelineState,
    pool: CVPixelBufferPool,
    texture_cache: CVMetalTextureCache,
}

impl MetalEngine {
    fn new() -> Result<Self, String> {
        let device = Device::system_default().ok_or("Metal device is unavailable")?;
        let source = include_str!("live_visual_metal.metal");
        let library = device
            .new_library_with_source(source, &metal::CompileOptions::new())
            .map_err(|error| format!("Metal shader compile: {error}"))?;
        let function = library
            .get_function("cadence_spectrogram", None)
            .map_err(|error| format!("Metal kernel lookup: {error}"))?;
        let pipeline = device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(|error| format!("Metal pipeline creation: {error}"))?;
        let queue = device.new_command_queue();
        let pool = make_pool()?;
        let texture_cache = texture_cache_for_device(&device)?;
        Ok(Self {
            device,
            queue,
            pipeline,
            pool,
            texture_cache,
        })
    }

    fn render(&self, request: &Request) -> Result<Option<CVPixelBuffer>, String> {
        let aux = dictionary(&[(
            CFString::from(CVPixelBufferPoolKeys::AllocationThreshold),
            cf_type(&CFNumber::from(POOL_LIMIT)),
        )]);
        let buffer = self
            .pool
            .create_pixel_buffer_with_aux_attributes(Some(&aux))
            .map_err(|status| (status, format!("CoreVideo frame allocation ({status})")));
        let buffer = match buffer {
            Ok(buffer) => buffer,
            Err((status, _)) if status == kCVReturnWouldExceedAllocationThreshold => {
                // CoreVideo's texture cache may retain old CVMetalTexture
                // wrappers after GPUI releases a scene. Flushing drops those
                // cache references; if GPUI still owns all pool buffers, skip
                // this frame and keep displaying the last completed surface.
                self.texture_cache.flush(0);
                return Ok(None);
            }
            Err((_, error)) => return Err(error),
        };

        let usage = CFNumber::from(
            (MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite).bits() as i32,
        );
        let texture_attributes =
            dictionary(&[(CFString::from(CVMetalTextureKeys::Usage), cf_type(&usage))]);
        let y_cv = self
            .texture_cache
            .create_texture_from_image(
                buffer.as_concrete_TypeRef(),
                Some(&texture_attributes),
                metal::MTLPixelFormat::R8Unorm,
                OUTPUT_WIDTH,
                OUTPUT_HEIGHT,
                0,
            )
            .map_err(|status| format!("CoreVideo luma texture ({status})"))?;
        let uv_cv = self
            .texture_cache
            .create_texture_from_image(
                buffer.as_concrete_TypeRef(),
                Some(&texture_attributes),
                metal::MTLPixelFormat::RG8Unorm,
                INPUT_WIDTH,
                INPUT_HEIGHT,
                1,
            )
            .map_err(|status| format!("CoreVideo chroma texture ({status})"))?;
        // CVMetalTextureGetTexture returns a borrowed MTLTexture. core-video
        // 0.4.3's get_texture() incorrectly transfers that borrow into an
        // owning wrapper; retain explicitly before converting it to owned.
        let y_texture = unsafe {
            let ptr = CVMetalTextureGetTexture(y_cv.as_concrete_TypeRef());
            if ptr.is_null() {
                return Err("CoreVideo returned an empty luma texture".into());
            }
            TextureRef::from_ptr(ptr).to_owned()
        };
        let uv_texture = unsafe {
            let ptr = CVMetalTextureGetTexture(uv_cv.as_concrete_TypeRef());
            if ptr.is_null() {
                return Err("CoreVideo returned an empty chroma texture".into());
            }
            TextureRef::from_ptr(ptr).to_owned()
        };

        // Metal copies this <=30 KiB input into its shared buffer. This is the
        // only per-frame CPU upload; palette, chronology, and frequency mapping
        // all run in the compute kernel.
        let input = self.device.new_buffer_with_data(
            request.values.as_ptr().cast(),
            request.values.len() as u64,
            MTLResourceOptions::StorageModeShared,
        );
        let command_buffer = self.queue.new_command_buffer();
        let encoder = command_buffer.new_compute_command_encoder();
        encoder.set_compute_pipeline_state(&self.pipeline);
        encoder.set_buffer(0, Some(&input), 0);
        let row_count = request.rows as u32;
        encoder.set_bytes(
            1,
            std::mem::size_of_val(&row_count) as u64,
            (&row_count as *const u32).cast(),
        );
        encoder.set_texture(0, Some(&y_texture));
        encoder.set_texture(1, Some(&uv_texture));
        encoder.dispatch_threads(
            MTLSize::new(INPUT_WIDTH as u64, INPUT_HEIGHT as u64, 1),
            MTLSize::new(16, 16, 1),
        );
        encoder.end_encoding();
        command_buffer.commit();
        // This blocks only the dedicated worker. Published buffers are never
        // reused; GPUI can retain each one without racing a later dispatch.
        command_buffer.wait_until_completed();
        if command_buffer.status() != metal::MTLCommandBufferStatus::Completed {
            return Err(format!(
                "Metal command ended with {:?}",
                command_buffer.status()
            ));
        }

        if std::env::var_os("CADENCE_GPU_DIAGNOSTICS").is_some() {
            eprintln!(
                "Cadence spectrogram GPU dispatch complete: device={}, cells={}x{}",
                self.device.name(),
                INPUT_WIDTH,
                INPUT_HEIGHT
            );
        }
        Ok(Some(buffer))
    }
}

fn texture_cache_for_device(device: &Device) -> Result<CVMetalTextureCache, String> {
    let mut cache: CVMetalTextureCacheRef = std::ptr::null_mut();
    let status = unsafe {
        create_metal_texture_cache(
            kCFAllocatorDefault,
            std::ptr::null(),
            device.as_ptr().cast(),
            std::ptr::null(),
            &mut cache,
        )
    };
    if status == kCVReturnSuccess {
        Ok(unsafe { TCFType::wrap_under_create_rule(cache) })
    } else {
        Err(format!("CoreVideo Metal texture cache ({status})"))
    }
}

fn make_pool() -> Result<CVPixelBufferPool, String> {
    let empty_surface = CFDictionary::<CFString, CFType>::from_CFType_pairs(&[]);
    let attributes = dictionary(&[
        (
            CFString::from(CVPixelBufferKeys::PixelFormatType),
            cf_type(&CFNumber::from(
                kCVPixelFormatType_420YpCbCr8BiPlanarFullRange as i32,
            )),
        ),
        (
            CFString::from(CVPixelBufferKeys::Width),
            cf_type(&CFNumber::from(OUTPUT_WIDTH as i32)),
        ),
        (
            CFString::from(CVPixelBufferKeys::Height),
            cf_type(&CFNumber::from(OUTPUT_HEIGHT as i32)),
        ),
        (
            CFString::from(CVPixelBufferKeys::MetalCompatibility),
            cf_type(&CFBoolean::true_value()),
        ),
        (
            CFString::from(CVPixelBufferKeys::IOSurfaceProperties),
            cf_type(&empty_surface),
        ),
    ]);
    CVPixelBufferPool::new(None, Some(&attributes))
        .map_err(|status| format!("CoreVideo frame pool creation ({status})"))
}

fn dictionary(pairs: &[(CFString, CFType)]) -> CFDictionary<CFString, CFType> {
    CFDictionary::from_CFType_pairs(pairs)
}

fn cf_type<T: TCFType>(value: &T) -> CFType {
    unsafe { CFType::wrap_under_get_rule(value.as_CFTypeRef()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_video::{pixel_buffer::kCVPixelBufferLock_ReadOnly, r#return::kCVReturnSuccess};

    #[test]
    fn frame_identity_reuses_only_within_same_source_generation_and_epoch() {
        let original = FrameKey {
            source: 0,
            generation: 7,
            epoch: 11,
            gpu_revision: 13,
        };
        let newer = FrameKey {
            gpu_revision: 14,
            ..original
        };
        assert!(original.same_source_epoch(newer));
        assert!(!original.same_source_epoch(FrameKey { source: 1, ..newer }));
        assert!(!original.same_source_epoch(FrameKey { epoch: 12, ..newer }));
        assert!(!original.same_source_epoch(FrameKey {
            generation: 8,
            ..newer
        }));
    }

    fn reconstructed_rgb(buffer: &CVPixelBuffer, x: usize, y: usize) -> [u8; 3] {
        assert_eq!(
            buffer.lock_base_address(kCVPixelBufferLock_ReadOnly),
            kCVReturnSuccess
        );
        let result = unsafe {
            let y_plane = buffer.get_base_address_of_plane(0).cast::<u8>();
            let uv_plane = buffer.get_base_address_of_plane(1).cast::<u8>();
            let y_stride = buffer.get_bytes_per_row_of_plane(0);
            let uv_stride = buffer.get_bytes_per_row_of_plane(1);
            let yy = *y_plane.add(y * 2 * y_stride + x * 2) as f32 / 255.0;
            let uv = uv_plane.add(y * uv_stride + x * 2);
            let cb = *uv as f32 / 255.0;
            let cr = *uv.add(1) as f32 / 255.0;
            [
                (yy + 1.402 * cr - 0.701).clamp(0.0, 1.0),
                (yy - 0.3441 * cb - 0.7141 * cr + 0.5291).clamp(0.0, 1.0),
                (yy + 1.772 * cb - 0.886).clamp(0.0, 1.0),
            ]
            .map(|channel| (channel * 255.0).round() as u8)
        };
        assert_eq!(
            buffer.unlock_base_address(kCVPixelBufferLock_ReadOnly),
            kCVReturnSuccess
        );
        result
    }

    #[test]
    fn metal_output_matches_cpu_palette_and_orientation_after_gpui_matrix() {
        autoreleasepool(|| {
            let engine = MetalEngine::new().expect("macOS test host should provide Metal");
            let rows = 2;
            let mut values = vec![0; rows * INPUT_HEIGHT];
            values[0] = 255; // Oldest row, lowest-frequency band.
            values[INPUT_HEIGHT * 2 - 1] = 128; // Newest row, highest-frequency band.
            let request = Request {
                key: FrameKey {
                    source: 0,
                    generation: 1,
                    epoch: 1,
                    gpu_revision: 1,
                },
                rows,
                values: values.into(),
            };
            let buffer = engine
                .render(&request)
                .expect("Metal dispatch should succeed")
                .expect("fresh pool should provide a frame");

            // GPU source row zero is right-aligned at x=238; low frequency is at
            // y=127. The latest row is x=239 with high frequency at y=0.
            for (x, y, magnitude) in [(238, 127, 255), (239, 0, 128), (0, 0, 0)] {
                let actual = reconstructed_rgb(&buffer, x, y);
                let bgra = crate::live_visual::magnitude_color(magnitude);
                let expected = [bgra[2], bgra[1], bgra[0]];
                for (actual, expected) in actual.into_iter().zip(expected) {
                    assert!(
                        actual.abs_diff(expected) <= 4,
                        "magnitude {magnitude}: GPU channel {actual} differs from CPU {expected}"
                    );
                }
            }

            // Completion must trigger a paint even when the audio frame is
            // paused. Repeated polls retain the completed surface and submit
            // no more work; a completion from the old source is rejected.
            let frame = crate::transport::LiveSpectrogramFrame::from_values(
                1,
                1,
                1,
                48_000,
                rows,
                Arc::clone(&request.values),
                vec![0; crate::transport::LIVE_SPECTRUM_POINT_COUNT].into(),
            )
            .unwrap();
            let key = FrameKey::new(0, &frame);
            let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
            let mut renderer = GpuSpectrogramRenderer {
                shared: Arc::clone(&shared),
                submitted: None,
                surface: None,
                failed: false,
            };
            assert!(!renderer.poll(0, Some(&frame)));
            {
                let mut state = shared.0.lock().unwrap();
                state.requested = None;
                state.ready = Some(Arc::new(PublishedSurface {
                    key,
                    buffer: buffer.clone(),
                }));
            }
            assert!(renderer.poll(0, Some(&frame)));
            assert!(!renderer.poll(0, Some(&frame)));
            assert_eq!(
                renderer.surface().unwrap().as_concrete_TypeRef(),
                buffer.as_concrete_TypeRef()
            );
            assert!(shared.0.lock().unwrap().requested.is_none());
            assert!(renderer.poll(1, Some(&frame)));
            shared.0.lock().unwrap().ready = Some(Arc::new(PublishedSurface { key, buffer }));
            assert!(!renderer.poll(1, Some(&frame)));
            assert!(renderer.surface().is_none());
        });
    }

    #[test]
    fn retained_surfaces_stay_immutable_and_pool_pressure_recovers() {
        autoreleasepool(|| {
            let engine = MetalEngine::new().unwrap();
            let mut retained = Vec::new();
            for magnitude in [0, 64, 128, 255] {
                let request = Request {
                    key: FrameKey {
                        source: 0,
                        generation: 1,
                        epoch: 1,
                        gpu_revision: magnitude as u64 + 1,
                    },
                    rows: 1,
                    values: vec![magnitude; INPUT_HEIGHT].into(),
                };
                retained.push(engine.render(&request).unwrap().unwrap());
            }
            let request = Request {
                key: FrameKey {
                    source: 0,
                    generation: 1,
                    epoch: 1,
                    gpu_revision: 300,
                },
                rows: 1,
                values: vec![192; INPUT_HEIGHT].into(),
            };
            assert!(engine.render(&request).unwrap().is_none());
            let first = retained.remove(0);
            let original = reconstructed_rgb(&first, 239, 127);
            assert!(
                original
                    .into_iter()
                    .zip([5u8, 5, 5])
                    .all(|(a, b)| a.abs_diff(b) <= 4)
            );
            drop(retained);
            engine.texture_cache.flush(0);
            assert!(engine.render(&request).unwrap().is_some());
            assert_eq!(reconstructed_rgb(&first, 239, 127), original);
        });
    }
}
