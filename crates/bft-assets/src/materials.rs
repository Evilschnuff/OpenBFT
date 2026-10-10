//! Engine-owned material descriptions.
//!
//! A [`MaterialDesc`] is what the `gamez` loader produces and
//! what `bft_game::materials::build_material` consumes. It is deliberately
//! renderer-free so `bft-sim` can read [`Soil`] off it for surface behaviour
//! without a render feature (docs/data/water.md).

use bevy::prelude::*;

use crate::mech3ax;
use crate::textures::texture_key;

pub use crate::mech3ax::Soil;

/// One material as the engine describes it.
#[derive(Debug, Clone, PartialEq)]
pub enum MaterialDesc {
    /// Samples a texture, looked up by name in a `TextureSet`/`TexturePool`.
    Textured {
        /// Already normalised by [`texture_key`].
        texture: String,
        soil: Soil,
        /// Baked cycling. `None` in every Recoil level; cycling comes from
        /// interp instead (docs/data/materials.md and docs/data/interp.md).
        cycle: Option<CycleDesc>,
        /// mech3ax's unexplained `flag`. `false` throughout Recoil.
        flag: bool,
    },
    /// A flat colour.
    Colored {
        /// Linear 0–1 RGB, converted from mech3ax's 0–255.
        color: [f32; 3],
        /// 0 = transparent, 255 = opaque.
        alpha: u8,
        soil: Soil,
    },
}

impl MaterialDesc {
    /// What this material's surface is made of.
    pub fn soil(&self) -> Soil {
        match self {
            MaterialDesc::Textured { soil, .. } | MaterialDesc::Colored { soil, .. } => *soil,
        }
    }

    /// The texture this material samples, if any.
    pub fn texture(&self) -> Option<&str> {
        match self {
            MaterialDesc::Textured { texture, .. } => Some(texture),
            MaterialDesc::Colored { .. } => None,
        }
    }
}

/// A run of texture frames played in sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct CycleDesc {
    /// Frame texture names, already normalised by [`texture_key`].
    pub textures: Vec<String>,
    pub looping: bool,
    /// Frames per second. `0.0` means the game advances the frame itself
    /// rather than time doing it (docs/data/interp.md: the tank-track set).
    pub speed: f32,
}

/// Errors converting a mech3ax material.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum MaterialError {
    #[error("material {material} names texture index {index}, but the level has only {count}")]
    TextureIndex {
        material: usize,
        index: u32,
        count: usize,
    },
}

/// Convert one mech3ax material, resolving its texture index against the
/// level's `textures.json`.
///
/// The index is the reason this cannot be a plain `From`: mech3ax's
/// `TexturedMaterial` stores `texture_index`, not a name (verified on all 13
/// levels), so the name table has to come along. `material` is the material's
/// own index, for the error message only.
pub fn material_desc(
    material: &mech3ax::Material,
    textures: &[mech3ax::TextureRef],
    material_index: usize,
) -> Result<MaterialDesc, MaterialError> {
    let name_of = |index: u32| -> Result<String, MaterialError> {
        textures
            .get(index as usize)
            .map(|texture| texture_key(&texture.name))
            .ok_or(MaterialError::TextureIndex {
                material: material_index,
                index,
                count: textures.len(),
            })
    };

    match material {
        mech3ax::Material::Textured(textured) => {
            let cycle = textured
                .cycle
                .as_ref()
                .map(|cycle| -> Result<CycleDesc, MaterialError> {
                    Ok(CycleDesc {
                        textures: cycle
                            .texture_indices
                            .iter()
                            .map(|index| name_of(*index))
                            .collect::<Result<_, _>>()?,
                        looping: cycle.looping,
                        speed: cycle.speed,
                    })
                })
                .transpose()?;

            Ok(MaterialDesc::Textured {
                texture: name_of(textured.texture_index)?,
                soil: textured.soil,
                cycle,
                flag: textured.flag,
            })
        }
        mech3ax::Material::Colored(colored) => Ok(MaterialDesc::Colored {
            color: [
                srgb_byte_to_linear(colored.color.r),
                srgb_byte_to_linear(colored.color.g),
                srgb_byte_to_linear(colored.color.b),
            ],
            alpha: colored.alpha,
            soil: colored.soil,
        }),
    }
}

/// Convert a whole level's material table.
///
/// This is what the `gamez` loader calls. `flag`'s meaning is unknown, so its
/// distribution is logged once
/// per level and nothing acts on it. In the retail data it is `false` on all
/// 4649 textured materials, so a level that logs any `true` is new
/// information.
pub fn material_descs(
    materials: &[mech3ax::Material],
    textures: &[mech3ax::TextureRef],
) -> Result<Vec<MaterialDesc>, MaterialError> {
    let descs = materials
        .iter()
        .enumerate()
        .map(|(index, material)| material_desc(material, textures, index))
        .collect::<Result<Vec<_>, _>>()?;

    let set = descs
        .iter()
        .filter(|desc| matches!(desc, MaterialDesc::Textured { flag: true, .. }))
        .count();
    let textured = descs
        .iter()
        .filter(|desc| matches!(desc, MaterialDesc::Textured { .. }))
        .count();

    if set > 0 {
        info!(
            "{set} of {textured} textured materials set the unknown `flag`; \
             the retail data never does, so this is worth investigating"
        );
    } else {
        debug!("`flag` is false on all {textured} textured materials, as expected");
    }

    Ok(descs)
}

/// mech3ax's 0–255 sRGB float to linear 0–1.
///
/// The components really are 0–255 despite being floats (verified: the
/// maximum across all 13 levels is exactly 255.0). Bevy's PBR pipeline wants
/// linear, and these are authored colours, so the sRGB transfer function
/// applies — the same one Bevy's `Color::srgb` uses.
fn srgb_byte_to_linear(component: f32) -> f32 {
    let value = (component / 255.0).clamp(0.0, 1.0);

    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mech3ax::{
        Color, ColoredMaterial, CycleData, Material, TextureRef, TexturedMaterial,
    };

    fn refs(names: &[&str]) -> Vec<TextureRef> {
        names
            .iter()
            .map(|name| TextureRef {
                name: (*name).to_string(),
            })
            .collect()
    }

    #[test]
    fn a_textured_material_resolves_its_index_to_a_stripped_name() {
        let textures = refs(&["lflare1", "dockp01k.tif", "m1gate01.tif"]);
        let material = Material::Textured(TexturedMaterial {
            texture_index: 1,
            soil: Soil::Water,
            cycle: None,
            flag: false,
        });

        let desc = material_desc(&material, &textures, 0).expect("converts");

        assert_eq!(
            desc,
            MaterialDesc::Textured {
                texture: "dockp01k".to_string(),
                soil: Soil::Water,
                cycle: None,
                flag: false,
            }
        );
        assert_eq!(desc.soil(), Soil::Water);
        assert_eq!(desc.texture(), Some("dockp01k"));
    }

    #[test]
    fn an_out_of_range_texture_index_names_the_material() {
        let material = Material::Textured(TexturedMaterial {
            texture_index: 9,
            ..default_textured()
        });

        let error = material_desc(&material, &refs(&["a", "b"]), 7).expect_err("out of range");

        assert_eq!(
            error,
            MaterialError::TextureIndex {
                material: 7,
                index: 9,
                count: 2,
            }
        );
    }

    #[test]
    fn baked_cycle_frames_resolve_through_the_same_table() {
        let material = Material::Textured(TexturedMaterial {
            texture_index: 0,
            cycle: Some(CycleData {
                texture_indices: vec![0, 2],
                looping: true,
                speed: 6.0,
            }),
            ..default_textured()
        });

        let desc =
            material_desc(&material, &refs(&["a.tif", "b.tif", "c.tif"]), 0).expect("converts");

        let MaterialDesc::Textured {
            cycle: Some(cycle), ..
        } = desc
        else {
            panic!("a cycled textured material");
        };
        assert_eq!(cycle.textures, vec!["a".to_string(), "c".to_string()]);
        assert!(cycle.looping);
    }

    #[test]
    fn colour_components_come_in_as_bytes_and_leave_linear() {
        let material = Material::Colored(ColoredMaterial {
            color: Color {
                r: 255.0,
                g: 0.0,
                b: 205.0,
            },
            alpha: 127,
            soil: Soil::Default,
        });

        let MaterialDesc::Colored { color, alpha, .. } =
            material_desc(&material, &[], 0).expect("converts")
        else {
            panic!("a coloured material");
        };

        assert_eq!(color[0], 1.0, "255 is full brightness, not 255x over");
        assert_eq!(color[1], 0.0);
        // Linear, so mid-grey sits well below the 205/255 sRGB value.
        assert!(color[2] > 0.5 && color[2] < 0.65, "{}", color[2]);
        assert_eq!(alpha, 127);
    }

    #[test]
    fn the_srgb_transfer_matches_bevys() {
        for byte in [0u8, 1, 64, 127, 205, 255] {
            let ours = srgb_byte_to_linear(f32::from(byte));
            let bevys = bevy::prelude::Color::srgb(f32::from(byte) / 255.0, 0.0, 0.0)
                .to_linear()
                .red;

            assert!(
                (ours - bevys).abs() < 1e-6,
                "byte {byte}: {ours} vs {bevys}"
            );
        }
    }

    #[test]
    fn the_batch_converter_keeps_order_and_reports_the_index_that_failed() {
        let textures = refs(&["a.tif", "b.tif"]);
        let materials = vec![
            Material::Textured(TexturedMaterial {
                texture_index: 1,
                ..default_textured()
            }),
            Material::Colored(ColoredMaterial {
                color: Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                },
                alpha: 255,
                soil: Soil::Default,
            }),
            Material::Textured(TexturedMaterial {
                texture_index: 0,
                ..default_textured()
            }),
        ];

        let descs = material_descs(&materials, &textures).expect("converts");

        assert_eq!(descs.len(), 3);
        assert_eq!(descs[0].texture(), Some("b"));
        assert_eq!(descs[1].texture(), None);
        assert_eq!(descs[2].texture(), Some("a"));

        // The second material is the bad one, and the error says so.
        let broken = vec![
            materials[1].clone(),
            Material::Textured(TexturedMaterial {
                texture_index: 5,
                ..default_textured()
            }),
        ];
        assert_eq!(
            material_descs(&broken, &textures).expect_err("out of range"),
            MaterialError::TextureIndex {
                material: 1,
                index: 5,
                count: 2,
            }
        );
    }

    fn default_textured() -> TexturedMaterial {
        TexturedMaterial {
            texture_index: 0,
            soil: Soil::Default,
            cycle: None,
            flag: false,
        }
    }
}
