//! Turning camera packets into RGBA frames.

use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use crate::{Encoding, Frame, MediaError, Result};

/// Decodes packets of one encoding into [`Frame`]s.
#[derive(Debug, Clone, Copy)]
pub struct Decoder {
    encoding: Encoding,
    width: u32,
    height: u32,
}

impl Decoder {
    pub fn new(encoding: Encoding, width: u32, height: u32) -> Self {
        Self { encoding, width, height }
    }

    /// Decode `packet` into `frame`, reusing its allocation.
    pub fn decode(&self, packet: &[u8], frame: &mut Frame) -> Result<()> {
        match self.encoding {
            Encoding::Mjpeg => decode_jpeg(packet, frame),
            Encoding::Yuyv => decode_yuyv(packet, self.width, self.height, frame),
        }
    }
}

fn decode_jpeg(packet: &[u8], frame: &mut Frame) -> Result<()> {
    let options = DecoderOptions::new_fast().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(packet), options);
    decoder.decode_headers().map_err(|e| MediaError::Decode(e.to_string()))?;
    let info = decoder.info().ok_or_else(|| MediaError::Decode("missing JPEG header".into()))?;
    let size = decoder.output_buffer_size().ok_or_else(|| MediaError::Decode("unknown JPEG size".into()))?;
    frame.width = info.width as u32;
    frame.height = info.height as u32;
    frame.rgba.resize(size, 0);
    decoder.decode_into(&mut frame.rgba).map_err(|e| MediaError::Decode(e.to_string()))
}

/// Packed YUYV 4:2:2 (BT.601, limited range) to RGBA.
fn decode_yuyv(packet: &[u8], width: u32, height: u32, frame: &mut Frame) -> Result<()> {
    let pixels = width as usize * height as usize;
    if packet.len() < pixels * 2 {
        return Err(MediaError::Decode(format!("short YUYV frame: {} bytes", packet.len())));
    }
    frame.width = width;
    frame.height = height;
    frame.rgba.resize(pixels * 4, 255);
    for (yuyv, rgba) in packet.chunks_exact(4).zip(frame.rgba.chunks_exact_mut(8)) {
        let (u, v) = (yuyv[1] as i32 - 128, yuyv[3] as i32 - 128);
        for (i, y) in [yuyv[0], yuyv[2]].into_iter().enumerate() {
            let c = 298 * (y as i32 - 16) + 128;
            let px = &mut rgba[i * 4..i * 4 + 4];
            px[0] = ((c + 409 * v) >> 8).clamp(0, 255) as u8;
            px[1] = ((c - 100 * u - 208 * v) >> 8).clamp(0, 255) as u8;
            px[2] = ((c + 516 * u) >> 8).clamp(0, 255) as u8;
            px[3] = 255;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuyv_grey_levels_and_colour() {
        // Two pixels: black then white, no chroma.
        let mut frame = Frame::default();
        decode_yuyv(&[16, 128, 235, 128], 2, 1, &mut frame).unwrap();
        assert_eq!(frame.rgba, [0, 0, 0, 255, 255, 255, 255, 255]);
        // Strong V (red difference) makes red dominate.
        decode_yuyv(&[128, 128, 128, 240], 2, 1, &mut frame).unwrap();
        assert!(frame.rgba[0] > frame.rgba[1] && frame.rgba[0] > frame.rgba[2]);
    }

    #[test]
    fn short_or_corrupt_input_is_an_error_not_a_panic() {
        let mut frame = Frame::default();
        assert!(decode_yuyv(&[0; 6], 2, 2, &mut frame).is_err());
        assert!(decode_jpeg(b"not a jpeg", &mut frame).is_err());
    }
}
