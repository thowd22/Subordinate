//! Golden test for the NV12 upload and YUV-to-RGB pass.
//!
//! A Rec.709 colour-bar frame is synthesised in limited-range NV12, uploaded,
//! converted on a real (usually software) wgpu device and read back, then
//! every bar's interior is compared against the reference RGB values the bars
//! are defined by. The same picture is run three ways — packed, stride-padded
//! and odd-sized — so a wrong stride, a swapped plane or a mishandled odd
//! dimension shows up as a colour, not as a crash.
//!
//! Where the machine has no wgpu adapter at all — a container with no ICD —
//! the test reports that and passes rather than failing the build on an
//! environment problem. Like `headless_context.rs`, one context is shared and
//! only one test touches the driver at a time: Mesa's lavapipe has segfaulted
//! when several threads drive devices at once.

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use sub_render::{Nv12Converter, Nv12Geometry, RenderContext, RenderError};

/// How far a converted channel may sit from its reference value.
///
/// Two codes of slack absorb the 8-bit quantisation of the sRGB target and
/// the small differences between GPUs' `pow` implementations; a swapped
/// channel or a plane misread by one row is off by tens of codes.
const TOLERANCE: i32 = 3;

/// Columns each side of a bar boundary that are left out of the comparison:
/// they carry a blend of both bars' chroma, which is correct 4:2:0 upsampling
/// rather than an error.
const EDGE: u32 = 2;

/// One colour bar: its limited-range Rec.709 YCbCr, and the full-range RGB a
/// correct conversion must produce.
struct Bar {
    name: &'static str,
    ycbcr: [u8; 3],
    rgb: [u8; 3],
}

/// The standard 100 % Rec.709 bars, plus a mid-grey patch that exercises the
/// middle of the transfer function rather than only its clipped ends.
const BARS: &[Bar] = &[
    Bar {
        name: "white",
        ycbcr: [235, 128, 128],
        rgb: [255, 255, 255],
    },
    Bar {
        name: "yellow",
        ycbcr: [219, 16, 138],
        rgb: [255, 255, 0],
    },
    Bar {
        name: "cyan",
        ycbcr: [188, 154, 16],
        rgb: [0, 255, 255],
    },
    Bar {
        name: "green",
        ycbcr: [173, 42, 26],
        rgb: [0, 255, 0],
    },
    Bar {
        name: "magenta",
        ycbcr: [78, 214, 230],
        rgb: [255, 0, 255],
    },
    Bar {
        name: "red",
        ycbcr: [63, 102, 240],
        rgb: [255, 0, 0],
    },
    Bar {
        name: "blue",
        ycbcr: [32, 240, 118],
        rgb: [0, 0, 255],
    },
    Bar {
        name: "grey",
        ycbcr: [126, 128, 128],
        rgb: [128, 128, 128],
    },
    Bar {
        name: "black",
        ycbcr: [16, 128, 128],
        rgb: [0, 0, 0],
    },
];

/// Held for the length of a test: only one test may talk to the driver.
static DRIVER: Mutex<()> = Mutex::new(());
/// The one context, built under `DRIVER`. `None` means no usable adapter.
static CONTEXT: OnceLock<Option<RenderContext>> = OnceLock::new();

/// Exclusive use of the shared context, or `None` when this machine has no
/// usable adapter. The guard must outlive every use of the context.
fn context_or_skip() -> Option<(MutexGuard<'static, ()>, RenderContext)> {
    let guard = DRIVER.lock().unwrap_or_else(PoisonError::into_inner);
    let context = CONTEXT.get_or_init(|| match RenderContext::headless() {
        Ok(context) => Some(context),
        Err(RenderError::NoAdapter { backends }) => {
            eprintln!("skipping: no wgpu adapter for backends [{backends}]");
            None
        }
        Err(error) => panic!("[{}] {error}", error.code()),
    });
    context.clone().map(|context| (guard, context))
}

/// Which bar covers pixel column `x` of a `width`-wide picture.
fn bar_at(x: u32, width: u32) -> usize {
    let bars = u32::try_from(BARS.len()).expect("nine bars fit in a u32");
    let index = x * bars / width;
    (index as usize).min(BARS.len() - 1)
}

/// Paint the colour bars into a fresh pair of NV12 planes.
///
/// Stride padding is filled with `0xA5` rather than left zero, so a copy that
/// wrongly treats the stride as the row length paints visible garbage into
/// the picture instead of black.
fn colour_bar_frame(geometry: Nv12Geometry) -> (Vec<u8>, Vec<u8>) {
    let mut y = vec![0xA5u8; geometry.y_plane_len()];
    for row in 0..geometry.height() {
        let base = row as usize * geometry.y_stride() as usize;
        for column in 0..geometry.width() {
            y[base + column as usize] = BARS[bar_at(column, geometry.width())].ycbcr[0];
        }
    }

    let mut uv = vec![0xA5u8; geometry.uv_plane_len()];
    for row in 0..geometry.chroma_height() {
        let base = row as usize * geometry.uv_stride() as usize;
        for column in 0..geometry.chroma_width() {
            // A chroma sample covers luma columns 2c and 2c+1; the bars are
            // wide and start on even columns, so either luma column names the
            // same bar.
            let bar = &BARS[bar_at((column * 2).min(geometry.width() - 1), geometry.width())];
            uv[base + column as usize * 2] = bar.ycbcr[1];
            uv[base + column as usize * 2 + 1] = bar.ycbcr[2];
        }
    }
    (y, uv)
}

/// Copy the converted picture back to the CPU as tightly packed RGBA.
fn read_back(context: &RenderContext, texture: &wgpu::Texture) -> Vec<u8> {
    let width = texture.width();
    let height = texture.height();
    let unpadded = width * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;

    let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("nv12 golden readback"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = context
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("nv12 golden readback"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    context.queue().submit([encoder.finish()]);

    buffer.slice(..).map_async(wgpu::MapMode::Read, |result| {
        result.expect("the readback buffer should map");
    });
    context
        .device()
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the readback should finish");

    let view = buffer
        .slice(..)
        .get_mapped_range()
        .expect("the mapped readback buffer should be readable");
    let mut pixels = Vec::with_capacity(unpadded as usize * height as usize);
    for row in 0..height {
        let start = row as usize * padded as usize;
        pixels.extend_from_slice(&view[start..start + unpadded as usize]);
    }
    drop(view);
    buffer.unmap();
    pixels
}

/// Convert a colour-bar frame of this geometry and check every bar interior
/// against its reference RGB.
fn check_colour_bars(context: &RenderContext, geometry: Nv12Geometry) {
    let device = context.device();
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let converter = Nv12Converter::new(device, geometry);
    let (y, uv) = colour_bar_frame(geometry);
    converter
        .submit_frame(device, context.queue(), &y, &uv)
        .expect("a frame built for this geometry should upload");
    let pixels = read_back(context, converter.output());

    let width = geometry.width();
    let mut checked = 0u32;
    for row in 0..geometry.height() {
        for column in 0..width {
            let bar = bar_at(column, width);
            let low = column.saturating_sub(EDGE);
            let high = (column + EDGE).min(width - 1);
            if bar_at(low, width) != bar || bar_at(high, width) != bar {
                continue;
            }
            let offset = (row as usize * width as usize + column as usize) * 4;
            let actual = &pixels[offset..offset + 4];
            let expected = BARS[bar].rgb;
            for (channel, name) in ["red", "green", "blue"].iter().enumerate() {
                let difference = i32::from(actual[channel]) - i32::from(expected[channel]);
                assert!(
                    difference.abs() <= TOLERANCE,
                    "{}x{} bar {:?} at ({column}, {row}): {name} is {}, expected {} (+-{TOLERANCE})",
                    geometry.width(),
                    geometry.height(),
                    BARS[bar].name,
                    actual[channel],
                    expected[channel],
                );
            }
            assert_eq!(actual[3], 255, "the converted frame must be opaque");
            checked += 1;
        }
    }
    assert!(
        checked > width * geometry.height() / 2,
        "most of the picture should have been compared, only {checked} pixels were"
    );

    let error = pollster::block_on(error_scope.pop());
    assert!(error.is_none(), "the conversion raised {error:?}");
}

#[test]
fn packed_colour_bars_convert_to_their_reference_colours() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let geometry = Nv12Geometry::packed(144, 32).expect("144x32 is a valid picture");
    check_colour_bars(&context, geometry);
}

#[test]
fn stride_padded_colour_bars_convert_to_the_same_colours() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    // What a hardware decoder hands back: rows padded out to 192 bytes.
    let geometry = Nv12Geometry::new(144, 32, 192, 192).expect("padded 144x32 is valid");
    check_colour_bars(&context, geometry);
}

#[test]
fn odd_sized_colour_bars_convert_to_the_same_colours() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    // Odd in both dimensions and padded: the chroma plane rounds up, so its
    // last column and row cover a single luma column and row.
    let geometry = Nv12Geometry::new(145, 33, 192, 192).expect("padded odd sizes are valid");
    check_colour_bars(&context, geometry);
}

#[test]
fn a_short_plane_is_refused_before_the_gpu_sees_it() {
    let Some((_driver, context)) = context_or_skip() else {
        return;
    };
    let geometry = Nv12Geometry::packed(144, 32).expect("144x32 is a valid picture");
    let converter = Nv12Converter::new(context.device(), geometry);
    let (y, uv) = colour_bar_frame(geometry);
    let error = converter
        .upload(context.queue(), &y[..y.len() - 1], &uv)
        .expect_err("a truncated luma plane must be refused");
    assert_eq!(error.code(), "render.short_plane");
    assert_eq!(converter.geometry(), geometry);
}
