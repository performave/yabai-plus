//! Live display discovery through CoreGraphics.
//!
//! This is intentionally a narrow Phase 5 boundary: it discovers display ids and
//! frames, leaving spaces, labels, and UUIDs to later SkyLight/CoreGraphics work.

#![cfg(target_os = "macos")]

use std::io;

use yabai_core::Area;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MacDisplay {
    pub id: u32,
    pub frame: Area,
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGGetActiveDisplayList(
        max_displays: u32,
        active_displays: *mut u32,
        display_count: *mut u32,
    ) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
}

pub fn active_displays() -> io::Result<Vec<MacDisplay>> {
    let mut count = 0u32;
    // SAFETY: `display_count` points to a valid `u32`; passing a null display
    // array with max 0 is the documented way to query the active display count.
    let err = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut count) };
    if err != 0 {
        return Err(io::Error::other(format!(
            "CGGetActiveDisplayList count failed with {err}"
        )));
    }

    let mut ids = vec![0u32; count as usize];
    // SAFETY: `ids` has room for `count` display ids and both pointers are valid
    // for writes for the duration of the call.
    let err = unsafe { CGGetActiveDisplayList(count, ids.as_mut_ptr(), &mut count) };
    if err != 0 {
        return Err(io::Error::other(format!(
            "CGGetActiveDisplayList ids failed with {err}"
        )));
    }
    ids.truncate(count as usize);

    Ok(ids
        .into_iter()
        .map(|id| {
            // SAFETY: `id` was returned by CoreGraphics as an active display id.
            let frame = unsafe { CGDisplayBounds(id) };
            MacDisplay {
                id,
                frame: Area::new(
                    frame.origin.x as f32,
                    frame.origin.y as f32,
                    frame.size.width as f32,
                    frame.size.height as f32,
                ),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_displays_call_is_safe() {
        let _ = active_displays().unwrap();
    }
}
