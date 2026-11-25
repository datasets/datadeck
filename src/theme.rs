#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    Editorial,
    Monochrome,
}

impl Theme {
    pub fn label(self) -> &'static str {
        match self {
            Self::Editorial => "Editorial",
            Self::Monochrome => "Monochrome",
        }
    }

    pub fn ink(self) -> u32 {
        0x161616
    }

    pub fn selection_fill(self) -> u32 {
        match self {
            Self::Editorial => 0xfaf1f4,
            Self::Monochrome => 0xf5f5f5,
        }
    }

    pub fn selection_rule(self) -> u32 {
        match self {
            Self::Editorial => 0x9b244c,
            Self::Monochrome => 0x262626,
        }
    }

    pub fn chart(self, index: usize) -> u32 {
        const PALETTE: [u32; 5] = [0xb94270, 0x4592ba, 0x9682b5, 0xbf873a, 0x65758b];
        match self {
            Self::Editorial => PALETTE[index % PALETTE.len()],
            Self::Monochrome => 0x272727,
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Editorial => Self::Monochrome,
            Self::Monochrome => Self::Editorial,
        }
    }
}
