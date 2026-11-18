//! A snapshot of a map, as MapLibre Native's snapshotter takes one: a still
//! map of its own on the app's runtime, with the map's style, content and
//! camera, rendered offscreen once everything it shows has loaded. A map has
//! one render session at a time, and the hosts' sessions draw to surfaces or
//! the hosts' textures, which can't be read back, so the map can't be read
//! itself.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mln_map::{CameraOptions, CameraUpdate, MapEvent, MapMode};
use mln_runtime::{AttachOptions, Completion, EventMask, Map, MapOptions, RenderSession, RenderTarget, Runtime};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::content::{self, Base, Sent};
use crate::model::{Content, Snapshot};
use crate::shared;

/// The largest `data:` URL a snapshot's record holds, which Exact's `image`
/// shows up to.
const LARGEST_URL: usize = 1 << 20;

/// What a snapshot is of.
pub(crate) struct Scene<'a> {
    pub(crate) style: &'a str,
    pub(crate) style_is_document: bool,
    pub(crate) camera: CameraOptions,
    pub(crate) content: &'a Content,
    pub(crate) images: &'a HashMap<String, PremultipliedImage>,
    /// Points, and image pixels in each.
    pub(crate) size: Size,
    pub(crate) pixel_ratio: f32,
}

enum Stage {
    /// Waiting for the style, whose base the content goes over.
    Loading,
    Base(Completion<String>),
    /// The content's commands, then the image once they ran.
    Applying(Vec<Sent>),
    Still(Completion<()>),
    Reading(Completion<PremultipliedImage>),
}

/// A snapshot on its way.
pub(crate) struct Snapshotter {
    map: Map,
    session: RenderSession,
    events: shared::Sink<MapEvent>,
    stage: Stage,
    content: Content,
    images: HashMap<String, PremultipliedImage>,
    size: Size,
    pixel_ratio: f32,
}

/// A finished snapshot: the PNG, and what the record has of it.
pub(crate) struct Taken {
    pub(crate) png: Vec<u8>,
    pub(crate) record: Snapshot,
}

impl Snapshotter {
    pub(crate) fn start(runtime: &Runtime, scene: &Scene<'_>) -> Result<Snapshotter, String> {
        let fail = |e: mln_runtime::Error| e.message;
        let options = MapOptions {
            map: mln_map::MapOptions {
                size: scene.size,
                pixel_ratio: scene.pixel_ratio,
                map_mode: MapMode::Static,
                ..Default::default()
            },
            event_mask: EventMask::ALL_MAP_EVENTS,
            ..Default::default()
        };
        let (map, created) = runtime.create_map(options).map_err(fail)?;
        let events = Arc::new(Mutex::new(Vec::new()));
        shared::follow(map.id(), events.clone(), Arc::new(Mutex::new(Vec::new())));
        let started = (|| {
            created.wait()?;
            let _jumped = map.update_camera(CameraUpdate { camera: scene.camera, ..Default::default() })?;
            let _loading = if scene.style_is_document {
                map.set_style_json(scene.style.to_owned())?
            } else {
                map.set_style_url(scene.style.to_owned())?
            };
            let target = RenderTarget::Offscreen { size: scene.size, device: None };
            let (session, _attached) = map.attach(target, AttachOptions::default())?;
            Ok(session)
        })();
        match started {
            Ok(session) => Ok(Snapshotter {
                map,
                session,
                events,
                stage: Stage::Loading,
                content: scene.content.clone(),
                images: scene.images.clone(),
                size: scene.size,
                pixel_ratio: scene.pixel_ratio,
            }),
            Err(e) => {
                shared::forget(map.id());
                let _ = map.release();
                Err(fail(e))
            }
        }
    }

    /// Moves the snapshot on: `None` while it is on its way, else the
    /// snapshot or why there is none.
    pub(crate) fn poll(&mut self) -> Option<Result<Taken, String>> {
        let events = std::mem::take(&mut *self.events.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for event in events {
            match event {
                MapEvent::StyleLoaded if matches!(self.stage, Stage::Loading) => match self.map.loaded_style_json() {
                    Ok(base) => self.stage = Stage::Base(base),
                    Err(e) => return Some(Err(e.message)),
                },
                MapEvent::LoadingFailed(error) => return Some(Err(format!("{error:?}"))),
                _ => {}
            }
        }
        loop {
            let next = match &mut self.stage {
                Stage::Loading => return None,
                Stage::Base(base) if base.is_complete() => {
                    let Stage::Base(base) = std::mem::replace(&mut self.stage, Stage::Loading) else { unreachable!() };
                    let base = match base.wait() {
                        Ok(json) => Base::parse(&json),
                        Err(e) => return Some(Err(e.message)),
                    };
                    let (_, sent) = content::apply(&self.map, &base, &Content::default(), &self.content, &self.images);
                    Stage::Applying(sent)
                }
                // A part of the content that fails is left out of the snapshot,
                // as the map leaves it out.
                Stage::Applying(sent) if sent.iter().all(|sent| sent.completion.is_complete()) => {
                    match self.map.request_still_image() {
                        Ok(still) => Stage::Still(still),
                        Err(e) => return Some(Err(e.message)),
                    }
                }
                Stage::Still(still) if still.is_complete() => {
                    let Stage::Still(still) = std::mem::replace(&mut self.stage, Stage::Loading) else {
                        unreachable!()
                    };
                    if let Err(e) = still.wait() {
                        return Some(Err(e.message));
                    }
                    match self.session.read_texture() {
                        Ok(pixels) => Stage::Reading(pixels),
                        Err(e) => return Some(Err(e.message)),
                    }
                }
                Stage::Reading(pixels) if pixels.is_complete() => {
                    let Stage::Reading(pixels) = std::mem::replace(&mut self.stage, Stage::Loading) else {
                        unreachable!()
                    };
                    return Some(pixels.wait().map(|pixels| self.taken(&pixels)).map_err(|e| e.message));
                }
                _ => return None,
            };
            self.stage = next;
        }
    }

    /// The PNG of `pixels`, and a `data:` URL of it for the record.
    fn taken(&self, pixels: &PremultipliedImage) -> Taken {
        let png = mln_image::encode_png(pixels);
        let (url, pixel_ratio) = fit(&png, pixels, self.size, self.pixel_ratio, LARGEST_URL);
        Taken { record: Snapshot { url, width: self.size.width, height: self.size.height, pixel_ratio }, png }
    }
}

/// A `data:` URL of `png`, the PNG of `pixels`, a snapshot of `size` points at
/// `pixel_ratio`, made smaller until it is at most `largest` long, and the
/// pixel ratio it then has.
fn fit(png: &[u8], pixels: &PremultipliedImage, size: Size, pixel_ratio: f32, largest: usize) -> (String, f32) {
    let (mut url, mut ratio, mut smaller) = (data_url(png), pixel_ratio, None::<PremultipliedImage>);
    while url.len() > largest {
        let image = smaller.as_ref().unwrap_or(pixels);
        if image.size.width <= 1 || image.size.height <= 1 {
            break;
        }
        let image = downscale(image, (largest as f64 / url.len() as f64).sqrt().min(0.9));
        ratio = image.size.width as f32 / size.width as f32;
        url = data_url(&mln_image::encode_png(&image));
        smaller = Some(image);
    }
    (url, ratio)
}

impl Drop for Snapshotter {
    fn drop(&mut self) {
        let _ = self.session.abandon();
        let _ = self.session.release();
        shared::forget(self.map.id());
        let _ = self.map.release();
    }
}

fn data_url(png: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64(png))
}

/// `bytes` in base64, as RFC 4648 writes it.
fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let [a, b, c] = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(a) << 16) | (u32::from(b) << 8) | u32::from(c);
        for i in 0..4 {
            if i <= chunk.len() {
                text.push(char::from(DIGITS[(n >> (18 - 6 * i)) as usize & 63]));
            } else {
                text.push('=');
            }
        }
    }
    text
}

/// `image` at `scale` of its size, each pixel the mean of those it covers.
fn downscale(image: &PremultipliedImage, scale: f64) -> PremultipliedImage {
    let (width, height) = (image.size.width as usize, image.size.height as usize);
    let to = |n: usize| ((n as f64 * scale).floor() as usize).max(1);
    let (new_width, new_height) = (to(width), to(height));
    let mut smaller = PremultipliedImage::new(Size::new(new_width as u32, new_height as u32));
    for y in 0..new_height {
        let rows = (y * height / new_height)..((y + 1) * height / new_height).max(y * height / new_height + 1);
        for x in 0..new_width {
            let columns = (x * width / new_width)..((x + 1) * width / new_width).max(x * width / new_width + 1);
            let mut sum = [0u32; 4];
            for row in rows.clone() {
                for column in columns.clone() {
                    let at = (row * width + column) * 4;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(image.data[at + channel]);
                    }
                }
            }
            let count = (rows.len() * columns.len()) as u32;
            let at = (y * new_width + x) * 4;
            for (channel, total) in sum.iter().enumerate() {
                smaller.data[at + channel] = (total / count) as u8;
            }
        }
    }
    smaller
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_is_rfc_4648s() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_snapshot_too_large_for_its_url_is_made_smaller_until_it_fits() {
        // Noise, which PNG can't compress.
        let mut pixels = PremultipliedImage::new(Size::new(128, 64));
        let mut seed = 12345u32;
        for byte in &mut pixels.data {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            *byte = (seed >> 16) as u8;
        }
        let png = mln_image::encode_png(&pixels);
        let (url, ratio) = fit(&png, &pixels, Size::new(64, 32), 2.0, 8 << 10);
        assert!(url.len() <= 8 << 10, "{}", url.len());
        assert!(ratio < 2.0 && ratio > 0.0, "{ratio}");
        let (whole, kept) = fit(&png, &pixels, Size::new(64, 32), 2.0, 1 << 20);
        assert_eq!((whole, kept), (data_url(&png), 2.0));
    }

    #[test]
    fn a_smaller_image_averages_the_pixels_it_covers() {
        let mut image = PremultipliedImage::new(Size::new(2, 2));
        image.data.copy_from_slice(&[0, 0, 0, 255, 200, 0, 0, 255, 0, 0, 0, 255, 200, 0, 0, 255]);
        let smaller = downscale(&image, 0.5);
        assert_eq!((smaller.size.width, smaller.size.height), (1, 1));
        assert_eq!(smaller.data, [100, 0, 0, 255]);
    }
}
