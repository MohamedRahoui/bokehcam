//! The fragment shaders, each with the textures it expects to be bound.
//!
//! A shader and the inputs it reads are declared together here, so the two
//! cannot drift apart between the WGSL source and the code that builds the
//! pass.

/// Shared by every shader: the vertex stage, the parameter block and the
/// sampler. See `shaders/common.wgsl`.
const COMMON: &str = include_str!("shaders/common.wgsl");

/// How a shader reads one of its input textures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Sampled with the linear sampler (8-bit and 16-bit float formats).
    Filtered,
    /// Read texel by texel (32-bit float formats, which cannot be filtered).
    Exact,
}

/// One fragment shader and its input textures, bound from binding 2 upward
/// in this order.
pub struct Shader {
    fragment: &'static str,
    pub inputs: &'static [Input],
}

impl Shader {
    /// The complete WGSL module: the shared part followed by the fragment stage.
    pub fn source(&self) -> String {
        format!("{COMMON}\n{}", self.fragment)
    }
}

const ONE_FILTERED: &[Input] = &[Input::Filtered];
const TWO_FILTERED: &[Input] = &[Input::Filtered; 2];
const ONE_EXACT: &[Input] = &[Input::Exact];

pub const FRAME: Shader = Shader { fragment: include_str!("shaders/frame.wgsl"), inputs: ONE_FILTERED };
pub const DOWN4: Shader = Shader { fragment: include_str!("shaders/down4.wgsl"), inputs: ONE_FILTERED };
pub const COPY: Shader = Shader { fragment: include_str!("shaders/copy.wgsl"), inputs: ONE_FILTERED };
pub const TEMPORAL: Shader = Shader { fragment: include_str!("shaders/temporal.wgsl"), inputs: TWO_FILTERED };
pub const GF_MOMENTS: Shader = Shader { fragment: include_str!("shaders/gf_moments.wgsl"), inputs: TWO_FILTERED };
pub const BOX: Shader = Shader { fragment: include_str!("shaders/box.wgsl"), inputs: ONE_EXACT };
pub const GF_COEFF: Shader = Shader { fragment: include_str!("shaders/gf_coeff.wgsl"), inputs: ONE_EXACT };
pub const BG_PREPARE: Shader = Shader { fragment: include_str!("shaders/bg_prepare.wgsl"), inputs: TWO_FILTERED };
pub const GAUSS: Shader = Shader { fragment: include_str!("shaders/gauss.wgsl"), inputs: ONE_FILTERED };
pub const COMPOSITE: Shader =
    Shader { fragment: include_str!("shaders/composite.wgsl"), inputs: &[Input::Filtered; 3] };
pub const YUYV: Shader = Shader { fragment: include_str!("shaders/yuyv.wgsl"), inputs: ONE_FILTERED };
