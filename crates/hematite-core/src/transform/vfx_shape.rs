//! VfxShapeFix transform.
//!
//! Complex VFX shape structure migration for post-patch 14.1 changes.
//! This restructures VFX shape embeds by moving BirthTranslation out of Shape
//! and converting field formats.
//!
//! ## What it does
//! 1. Find `VfxSystemDefinitionData` objects
//! 2. Look for emitter containers (Complex/SimpleEmitterDefinitionData)
//! 3. Analyze Shape embeds (stored as `Embedded`) for old-format fields
//! 4. Convert shape struct type (0x3dbe415d, 0xee39916f, or 0x4f4e2ed7)
//! 5. Change the Shape property from `Embedded` → `Struct` and rename its
//!    field hash to `NEW_SHAPE_HASH` (0x3bf0b4ed) — this is required by the game
//! 6. Move `BirthTranslation` from inside Shape to outside as sibling field
//!
//! ## Shape type constants (class_hash of the converted shape struct)
//! - `0x3dbe415d` — Cylinder with Radius/Height/Flags
//! - `0xee39916f` — Simple EmitOffset (Vec3)
//! - `0x4f4e2ed7` — Default fallback (empty)
//!
//! ## Field hash constants
//! - `0x3bf0b4ed` — New field hash for the Shape property on the emitter
//!   (replaces the old "Shape" FNV-1a hash; game expects this after patch 14.1)

use crate::context::FixContext;
use hematite_types::bin::{BinProperty, PropertyValue, StructValue};
use hematite_types::hash::{FieldHash, TypeHash};

/// Field hash the game expects for the shape property after patch 14.1.
/// The old hash was FNV-1a("shape"); the new one is different and must be used.
const NEW_SHAPE_HASH: u32 = 0x3bf0b4ed;

const NEW_BIRTH_TRANSLATION_HASH: u32 = 0x563d4a22;
const BIRTH_TRANSLATION_TYPE_HASH: u32 = 0x68dc32b6;
const SHAPE_TYPE_CYLINDER: u32 = 0x3dbe415d;
const SHAPE_TYPE_SIMPLE: u32 = 0xee39916f;
const SHAPE_TYPE_DEFAULT: u32 = 0x4f4e2ed7;

struct ShapeAnalysis {
    needs_fix: bool,
    birth_translation_vec3: Option<[f32; 3]>,
    radius: f32,
    height: f32,
    /// True when EmitRotationAngles field is present (one condition for cylinder).
    has_emit_rotation_angles: bool,
    /// True when EmitRotationAxes has the {0,1,0}{0,0,1} cylinder axis pattern.
    has_cylinder_axes_pattern: bool,
    /// True when EmitOffset is the only non-BirthTranslation property → simple shape.
    has_single_emit_offset: bool,
}

pub fn apply(ctx: &mut FixContext, entry_type: &str) -> u32 {
    let Some(vfx_system_hash) = ctx.hashes.type_hash(entry_type) else {
        return 0;
    };
    let Some(complex_emitter_hash) = ctx.hashes.type_hash("ComplexEmitterDefinitionData") else {
        return 0;
    };
    let Some(simple_emitter_hash) = ctx.hashes.type_hash("SimpleEmitterDefinitionData") else {
        return 0;
    };
    // Sometimes custom skins just use the base class or ParticleEmitterProperties directly
    let vfx_emitter_hash = ctx.hashes.type_hash("VfxEmitterDefinitionData");
    let particle_emitter_hash = ctx.hashes.type_hash("ParticleEmitterProperties");

    let hashes = VfxHashes {
        shape: ctx.hashes.field_hash("Shape"),
        birth_translation: ctx.hashes.field_hash("BirthTranslation"),
        emit_offset: ctx.hashes.field_hash("EmitOffset"),
        emit_rotation_angles: ctx.hashes.field_hash("EmitRotationAngles"),
        emit_rotation_axes: ctx.hashes.field_hash("EmitRotationAxes"),
        constant_value: ctx.hashes.field_hash("ConstantValue"),
        radius: ctx.hashes.field_hash("Radius"),
        height: ctx.hashes.field_hash("Height"),
        flags: ctx.hashes.field_hash("Flags"),
    };

    let Some(shape_hash) = hashes.shape else {
        return 0;
    };

    let mut changes = 0u32;
    let object_keys: Vec<u32> = ctx
        .tree
        .objects
        .keys()
        .filter(|&&ph| {
            ctx.tree
                .objects
                .get(&ph)
                .map(|o| o.class_hash == vfx_system_hash)
                .unwrap_or(false)
        })
        .copied()
        .collect();

    for path_hash in object_keys {
        let Some(obj) = ctx.tree.objects.get_mut(&path_hash) else {
            continue;
        };

        let prop_keys: Vec<u32> = obj.properties.keys().copied().collect();

        for prop_hash in prop_keys {
            let Some(prop) = obj.properties.get_mut(&prop_hash) else {
                continue;
            };

            if let PropertyValue::Container(emitters) | PropertyValue::UnorderedContainer(emitters) = &mut prop.value {
                for emitter_val in emitters.iter_mut() {
                    let emitter = match emitter_val {
                        PropertyValue::Embedded(e) | PropertyValue::Struct(e) => e,
                        _ => continue,
                    };
                    
                    let is_emitter = emitter.class_hash == complex_emitter_hash
                        || emitter.class_hash == simple_emitter_hash
                        || vfx_emitter_hash.map(|h| h == emitter.class_hash).unwrap_or(false)
                        || particle_emitter_hash.map(|h| h == emitter.class_hash).unwrap_or(false);
                        
                    if !is_emitter {
                        continue;
                    }

                        // Swap-remove the shape property so we can own and modify it,
                        // then re-insert under the new field hash.
                        let old_shape_prop = emitter.properties.swap_remove(&shape_hash.0);
                        if let Some(old_prop) = old_shape_prop {
                            let was_struct = matches!(old_prop.value, PropertyValue::Struct(_));
                            if let PropertyValue::Embedded(mut shape) | PropertyValue::Struct(mut shape) = old_prop.value {
                                let analysis = analyze_shape(&shape, &hashes);

                                if analysis.needs_fix {
                                    apply_shape_conversion(&mut shape, &analysis, &hashes);

                                    if let Some(birth_vec) = analysis.birth_translation_vec3 {
                                        move_birth_translation_outside(
                                            emitter,
                                            birth_vec,
                                            hashes.constant_value,
                                        );
                                    }

                                    // Re-insert under new field hash, as Struct (not Embedded).
                                    emitter.properties.insert(
                                        NEW_SHAPE_HASH,
                                        BinProperty {
                                            name_hash: FieldHash(NEW_SHAPE_HASH),
                                            value: PropertyValue::Struct(shape),
                                        },
                                    );
                                    changes += 1;
                                } else {
                                    // No fix needed — restore original.
                                    emitter.properties.insert(
                                        shape_hash.0,
                                        BinProperty {
                                            name_hash: shape_hash,
                                            value: if was_struct { PropertyValue::Struct(shape) } else { PropertyValue::Embedded(shape) },
                                        },
                                    );
                                }
                            } else {
                                // Not an Embedded/Struct value (e.g. already converted to something else?) — restore.
                                emitter.properties.insert(shape_hash.0, old_prop);
                            }
                        }
                    // End of emitter block
                }
            }
        }
    }

    changes
}

struct VfxHashes {
    shape: Option<FieldHash>,
    birth_translation: Option<FieldHash>,
    emit_offset: Option<FieldHash>,
    emit_rotation_angles: Option<FieldHash>,
    emit_rotation_axes: Option<FieldHash>,
    constant_value: Option<FieldHash>,
    radius: Option<FieldHash>,
    height: Option<FieldHash>,
    flags: Option<FieldHash>,
}

fn analyze_shape(shape: &StructValue, hashes: &VfxHashes) -> ShapeAnalysis {
    let mut analysis = ShapeAnalysis {
        needs_fix: false,
        birth_translation_vec3: None,
        radius: 0.0,
        height: 0.0,
        has_emit_rotation_angles: false,
        has_cylinder_axes_pattern: false,
        has_single_emit_offset: false,
    };

    let mut has_birth_translation = false;
    let mut has_emit_offset = false;

    for (field_hash, field_prop) in &shape.properties {
        if hashes
            .birth_translation
            .map(|h| *field_hash == h.0)
            .unwrap_or(false)
        {
            analysis.needs_fix = true;
            has_birth_translation = true;
            if let PropertyValue::Struct(bt_struct) | PropertyValue::Embedded(bt_struct) =
                &field_prop.value
            {
                analysis.birth_translation_vec3 =
                    extract_constant_value_vec3(bt_struct, hashes.constant_value);
            }
        }

        if hashes
            .emit_offset
            .map(|h| *field_hash == h.0)
            .unwrap_or(false)
        {
            analysis.needs_fix = true;
            has_emit_offset = true;
            if let PropertyValue::Struct(eo_struct) | PropertyValue::Embedded(eo_struct) =
                &field_prop.value
            {
                if let Some(vec3) = extract_constant_value_vec3(eo_struct, hashes.constant_value) {
                    analysis.radius = vec3[0];
                    analysis.height = vec3[1];
                }
            }
        }

        if hashes
            .emit_rotation_angles
            .map(|h| *field_hash == h.0)
            .unwrap_or(false)
        {
            analysis.needs_fix = true;
            analysis.has_emit_rotation_angles = true;
        }

        if hashes
            .emit_rotation_axes
            .map(|h| *field_hash == h.0)
            .unwrap_or(false)
        {
            analysis.needs_fix = true;
            if let PropertyValue::Container(axes) = &field_prop.value {
                if axes.len() == 2 {
                    if let (PropertyValue::Vector3(v0), PropertyValue::Vector3(v1)) =
                        (&axes[0], &axes[1])
                    {
                        // Pattern: { 0, 1, 0 } { 0, 0, 1 } — the cylinder axis pattern
                        if v0[1] == 1.0
                            && v0[0] == 0.0
                            && v0[2] == 0.0
                            && v1[2] == 1.0
                            && v1[0] == 0.0
                            && v1[1] == 0.0
                        {
                            analysis.has_cylinder_axes_pattern = true;
                        }
                    }
                }
            }
        }
    }

    // Determine if this is a single-EmitOffset shape (becomes SHAPE_TYPE_SIMPLE).
    // A shape qualifies if EmitOffset is the only field besides BirthTranslation.
    let non_birth_count = shape.properties.len() - if has_birth_translation { 1 } else { 0 };
    analysis.has_single_emit_offset = has_emit_offset && non_birth_count == 1;

    analysis
}

fn extract_constant_value_vec3(
    struct_val: &StructValue,
    constant_value_hash: Option<FieldHash>,
) -> Option<[f32; 3]> {
    let cv_hash = constant_value_hash?;
    let prop = struct_val.properties.get(&cv_hash.0)?;
    if let PropertyValue::Vector3(vec) = &prop.value {
        Some(*vec)
    } else {
        None
    }
}

fn apply_shape_conversion(shape: &mut StructValue, analysis: &ShapeAnalysis, hashes: &VfxHashes) {
    // Cylinder requires BOTH EmitRotationAngles AND the {0,1,0}{0,0,1} axis pattern.
    let target_type = if analysis.has_emit_rotation_angles
        && analysis.has_cylinder_axes_pattern
        && analysis.radius != 0.0
    {
        SHAPE_TYPE_CYLINDER
    } else if analysis.has_single_emit_offset {
        SHAPE_TYPE_SIMPLE
    } else {
        SHAPE_TYPE_DEFAULT
    };

    shape.properties.clear();
    shape.class_hash = TypeHash(target_type);

    match target_type {
        SHAPE_TYPE_CYLINDER => {
            if let Some(r_hash) = hashes.radius {
                shape.properties.insert(
                    r_hash.0,
                    BinProperty {
                        name_hash: r_hash,
                        value: PropertyValue::F32(analysis.radius),
                    },
                );
            }
            if analysis.height != 0.0 {
                if let Some(h_hash) = hashes.height {
                    shape.properties.insert(
                        h_hash.0,
                        BinProperty {
                            name_hash: h_hash,
                            value: PropertyValue::F32(analysis.height),
                        },
                    );
                }
            }
            if let Some(f_hash) = hashes.flags {
                shape.properties.insert(
                    f_hash.0,
                    BinProperty {
                        name_hash: f_hash,
                        value: PropertyValue::U8(1),
                    },
                );
            }
        }
        SHAPE_TYPE_SIMPLE => {
            // Simple shape: one EmitOffset field containing the radius/height as a Vec3.
            // Field hash stays as EmitOffset, but value becomes a plain Vector3
            // (no longer wrapped in a struct with ConstantValue).
            if let Some(eo_hash) = hashes.emit_offset {
                shape.properties.insert(
                    eo_hash.0,
                    BinProperty {
                        name_hash: eo_hash,
                        value: PropertyValue::Vector3([analysis.radius, analysis.height, 0.0]),
                    },
                );
            }
        }
        SHAPE_TYPE_DEFAULT => {}
        _ => {}
    }
}

fn move_birth_translation_outside(
    emitter: &mut StructValue,
    birth_vec: [f32; 3],
    constant_value_hash: Option<FieldHash>,
) {
    let Some(cv_hash) = constant_value_hash else {
        return;
    };

    let mut birth_props = indexmap::IndexMap::new();
    birth_props.insert(
        cv_hash.0,
        BinProperty {
            name_hash: cv_hash,
            value: PropertyValue::Vector3(birth_vec),
        },
    );

    emitter.properties.insert(
        NEW_BIRTH_TRANSLATION_HASH,
        BinProperty {
            name_hash: FieldHash(NEW_BIRTH_TRANSLATION_HASH),
            value: PropertyValue::Embedded(StructValue {
                class_hash: TypeHash(BIRTH_TRANSLATION_TYPE_HASH),
                properties: birth_props,
            }),
        },
    );
}
