//! Shared colors for reader chrome and native text inputs. Book typography is
//! independent of these controls.
use gpui::{Hsla, rgb};

#[derive(Clone, Copy)]
pub(super) struct ReaderPalette {
    pub surface: Hsla,
    pub control: Hsla,
    pub hover: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub selected: Hsla,
    pub accent: Hsla,
}

impl ReaderPalette {
    pub fn new(light: bool) -> Self {
        let colors = if light {
            [
                0xf5f4f1, 0xeae9e5, 0xe1e0dc, 0xd8d7d3, 0x202020, 0x66645f, 0xdce9f5, 0x0759b5,
            ]
        } else {
            [
                0x252525, 0x303030, 0x3a3a3a, 0x414141, 0xe8e6e3, 0xaaa8a4, 0x293f55, 0x8ebaff,
            ]
        };
        let [
            surface,
            control,
            hover,
            border,
            text,
            muted,
            selected,
            accent,
        ] = colors.map(|color| rgb(color).into());
        Self {
            surface,
            control,
            hover,
            border,
            text,
            muted,
            selected,
            accent,
        }
    }
}
