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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use wgpu::naga::valid::{Capabilities, ValidationFlags, Validator};
    use wgpu::naga::{Expression, Function, Module, ShaderStage, TypeInner, front::wgsl};

    use super::*;

    /// Every shader, under the name of its file in `shaders/`.
    const ALL: &[(&str, &Shader)] = &[
        ("frame", &FRAME),
        ("down4", &DOWN4),
        ("copy", &COPY),
        ("temporal", &TEMPORAL),
        ("gf_moments", &GF_MOMENTS),
        ("box", &BOX),
        ("gf_coeff", &GF_COEFF),
        ("bg_prepare", &BG_PREPARE),
        ("gauss", &GAUSS),
        ("composite", &COMPOSITE),
        ("yuyv", &YUYV),
    ];

    /// The first binding after the parameter block and the sampler.
    const FIRST_TEXTURE_BINDING: u32 = 2;

    /// What a graphics device would reject when building a pass from this
    /// shader, found without one.
    fn check(shader: &Shader) -> Result<(), String> {
        let source = shader.source();
        let module = wgsl::parse_str(&source).map_err(|e| e.emit_to_string(&source))?;
        Validator::new(ValidationFlags::all(), Capabilities::empty())
            .validate(&module)
            .map_err(|e| e.emit_to_string(&source))?;

        for (name, stage) in [("vs", ShaderStage::Vertex), ("fs", ShaderStage::Fragment)] {
            if !module.entry_points.iter().any(|entry| entry.name == name && entry.stage == stage) {
                return Err(format!("no {stage:?} entry point named `{name}`"));
            }
        }

        let declared: BTreeSet<u32> = textures(&module).map(|(binding, _)| binding).collect();
        let expected: BTreeSet<u32> = (FIRST_TEXTURE_BINDING..).take(shader.inputs.len()).collect();
        if declared != expected {
            return Err(format!("textures are declared at bindings {declared:?}, the pass binds {expected:?}"));
        }

        let functions = module.functions.iter().map(|(_, f)| f).chain(module.entry_points.iter().map(|e| &e.function));
        for binding in functions.flat_map(|function| sampled_bindings(&module, function)) {
            let binding = binding?;
            if shader.inputs[(binding - FIRST_TEXTURE_BINDING) as usize] == Input::Exact {
                return Err(format!("the texture at binding {binding} is sampled but bound as `Input::Exact`"));
            }
        }
        Ok(())
    }

    /// The textures a module declares: binding and name.
    fn textures(module: &Module) -> impl Iterator<Item = (u32, &str)> {
        module.global_variables.iter().filter_map(|(_, variable)| {
            let is_texture = matches!(module.types[variable.ty].inner, TypeInner::Image { .. });
            let binding = variable.binding.as_ref()?.binding;
            is_texture.then_some((binding, variable.name.as_deref().unwrap_or_default()))
        })
    }

    /// The binding of every texture this function samples through the sampler.
    fn sampled_bindings<'a>(
        module: &'a Module,
        function: &'a Function,
    ) -> impl Iterator<Item = Result<u32, String>> + 'a {
        function.expressions.iter().filter_map(move |(_, expression)| {
            let Expression::ImageSample { image, .. } = expression else { return None };
            Some(match function.expressions[*image] {
                Expression::GlobalVariable(variable) => module.global_variables[variable]
                    .binding
                    .as_ref()
                    .map(|binding| binding.binding)
                    .ok_or_else(|| "a sampled texture has no binding".to_string()),
                _ => Err("a texture is sampled through something other than its global; this check cannot follow it"
                    .to_string()),
            })
        })
    }

    fn broken(fragment: &'static str, inputs: &'static [Input]) -> Shader {
        Shader { fragment, inputs }
    }

    #[test]
    fn every_shader_is_valid_and_matches_its_declared_inputs() {
        for (name, shader) in ALL {
            if let Err(problem) = check(shader) {
                panic!("shaders/{name}.wgsl: {problem}");
            }
        }
    }

    #[test]
    fn every_shader_file_is_checked() {
        let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/src/shaders");
        let on_disk: BTreeSet<String> = std::fs::read_dir(directory)
            .expect("the shader directory is readable")
            .map(|entry| entry.expect("a directory entry").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "wgsl"))
            .map(|path| path.file_stem().expect("a file name").to_string_lossy().into_owned())
            .filter(|name| name != "common")
            .collect();
        let checked: BTreeSet<String> = ALL.iter().map(|(name, _)| name.to_string()).collect();
        assert_eq!(on_disk, checked);
    }

    #[test]
    fn a_syntax_error_is_reported() {
        let shader = broken("@fragment fn fs(in: VsOut) -> @location(0) vec4f { return vec4f(1.0) + ; }", &[]);
        assert!(check(&shader).is_err());
    }

    #[test]
    fn a_type_error_is_reported() {
        let shader = broken("@fragment fn fs(in: VsOut) -> @location(0) vec4f { return 1.0; }", &[]);
        assert!(check(&shader).is_err());
    }

    #[test]
    fn a_missing_fragment_stage_is_reported() {
        let problem = check(&broken("", &[])).unwrap_err();
        assert!(problem.contains("`fs`"), "{problem}");
    }

    #[test]
    fn a_texture_the_pass_does_not_bind_is_reported() {
        let shader = broken(
            "@group(0) @binding(2) var a: texture_2d<f32>;
             @group(0) @binding(3) var b: texture_2d<f32>;
             @fragment fn fs(in: VsOut) -> @location(0) vec4f {
                 return textureSample(a, samp, in.uv) + textureSample(b, samp, in.uv);
             }",
            ONE_FILTERED,
        );
        let problem = check(&shader).unwrap_err();
        assert!(problem.contains("bindings"), "{problem}");
    }

    #[test]
    fn a_bound_texture_the_shader_does_not_declare_is_reported() {
        let shader = broken(
            "@group(0) @binding(2) var a: texture_2d<f32>;
             @fragment fn fs(in: VsOut) -> @location(0) vec4f { return textureSample(a, samp, in.uv); }",
            TWO_FILTERED,
        );
        let problem = check(&shader).unwrap_err();
        assert!(problem.contains("bindings"), "{problem}");
    }

    #[test]
    fn sampling_a_texture_bound_as_exact_is_reported() {
        let shader = broken(
            "@group(0) @binding(2) var a: texture_2d<f32>;
             @fragment fn fs(in: VsOut) -> @location(0) vec4f { return textureSample(a, samp, in.uv); }",
            ONE_EXACT,
        );
        let problem = check(&shader).unwrap_err();
        assert!(problem.contains("Input::Exact"), "{problem}");
    }

    #[test]
    fn loading_texels_from_a_texture_bound_as_exact_is_accepted() {
        let shader = broken(
            "@group(0) @binding(2) var a: texture_2d<f32>;
             @fragment fn fs(in: VsOut) -> @location(0) vec4f { return textureLoad(a, vec2i(in.pos.xy), 0); }",
            ONE_EXACT,
        );
        assert_eq!(check(&shader), Ok(()));
    }
}
