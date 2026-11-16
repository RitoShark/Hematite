use crate::context::FixContext;
use crate::strings::fnv1a_hash as hash;
use hematite_types::bin::{BinProperty, BinTree, PropertyValue, StructValue};
use hematite_types::hash::{FieldHash, TypeHash};
use indexmap::IndexMap;

pub fn apply(ctx: &mut FixContext, entry_type: &str) -> u32 {
    repair(&mut ctx.tree, entry_type)
}

pub fn detect(tree: &BinTree, entry_type: &str) -> bool {
    repair(&mut tree.clone(), entry_type) > 0
}

fn repair(tree: &mut BinTree, entry_type: &str) -> u32 {
    tree.objects
        .values_mut()
        .filter(|object| object.class_hash.0 == hash(entry_type))
        .map(|object| walk_fields(&mut object.properties))
        .sum()
}

fn walk_fields(fields: &mut IndexMap<u32, BinProperty>) -> u32 {
    fields.values_mut().map(|prop| walk(&mut prop.value)).sum()
}

fn walk(value: &mut PropertyValue) -> u32 {
    match value {
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            let mut count = if [
                "VfxEmitterDefinitionData",
                "ComplexEmitterDefinitionData",
                "SimpleEmitterDefinitionData",
                "ParticleEmitterProperties",
            ]
            .iter()
            .any(|name| hash(name) == s.class_hash.0)
            {
                repair_emitter(s)
            } else {
                0
            };
            count += walk_fields(&mut s.properties);
            count
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            items.iter_mut().map(walk).sum()
        }
        PropertyValue::Map(items) => items
            .iter_mut()
            .map(|(key, value)| walk(key) + walk(value))
            .sum(),
        PropertyValue::Optional(value) => value.as_mut().as_mut().map(walk).unwrap_or(0),
        _ => 0,
    }
}

fn move_position(fields: &mut IndexMap<u32, BinProperty>, mut prop: BinProperty) {
    let position = hash("EmitterPosition");
    prop.name_hash = FieldHash(position);
    fields.entry(position).or_insert(prop);
}

fn repair_emitter(emitter: &mut StructValue) -> u32 {
    let fields = &mut emitter.properties;
    let shape = hash("shape");
    let spawn = hash("SpawnShape");
    let translation = hash("birthTranslation");
    let legacy = hash("VfxShapeLegacy");
    let mut count = 0;
    let valid_old = fields.get(&shape).is_some_and(|prop| match &prop.value {
        PropertyValue::Embedded(s) | PropertyValue::Struct(s) => s.class_hash.0 != 0,
        _ => false,
    });
    if valid_old {
        let prop = fields.shift_remove(&shape).unwrap();
        let mut old = match prop.value {
            PropertyValue::Embedded(s) | PropertyValue::Struct(s) => s,
            _ => unreachable!(),
        };
        if let Some(position) = old.properties.shift_remove(&translation) {
            move_position(fields, position);
        }
        if !old.properties.is_empty() {
            old.class_hash = TypeHash(legacy);
            fields.entry(spawn).or_insert(BinProperty {
                name_hash: FieldHash(spawn),
                value: PropertyValue::Struct(old),
            });
        }
        count += 1;
    }
    let misplaced = fields
        .get_mut(&spawn)
        .and_then(|prop| match &mut prop.value {
            PropertyValue::Struct(s) | PropertyValue::Embedded(s) if s.class_hash.0 == legacy => {
                s.properties.shift_remove(&translation)
            }
            _ => None,
        });
    if let Some(position) = misplaced {
        move_position(fields, position);
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(name: &str, value: PropertyValue) -> (u32, BinProperty) {
        let key = hash(name);
        (
            key,
            BinProperty {
                name_hash: FieldHash(key),
                value,
            },
        )
    }

    fn structure(name: &str, fields: Vec<(u32, BinProperty)>) -> StructValue {
        StructValue {
            class_hash: TypeHash(hash(name)),
            properties: fields.into_iter().collect(),
        }
    }

    #[test]
    fn preserves_curves_unknown_fields_and_all_offset_components() {
        let curve = PropertyValue::Embedded(structure(
            "ValueVector3",
            vec![
                prop("constantValue", PropertyValue::Vector3([1.0, 2.0, 3.0])),
                prop(
                    "dynamics",
                    PropertyValue::Container(vec![PropertyValue::F32(7.0)]),
                ),
            ],
        ));
        let mut emitter = structure(
            "VfxEmitterDefinitionData",
            vec![prop(
                "shape",
                PropertyValue::Embedded(structure(
                    "VfxShape",
                    vec![
                        prop("birthTranslation", curve),
                        prop("emitOffset", PropertyValue::Vector3([4.0, 5.0, 6.0])),
                        prop("unknown", PropertyValue::U32(42)),
                    ],
                )),
            )],
        );
        assert_eq!(repair_emitter(&mut emitter), 1);
        let PropertyValue::Embedded(position) = &emitter.properties[&hash("EmitterPosition")].value
        else {
            panic!()
        };
        assert!(position.properties.contains_key(&hash("dynamics")));
        let PropertyValue::Struct(shape) = &emitter.properties[&hash("SpawnShape")].value else {
            panic!()
        };
        assert_eq!(shape.class_hash.0, hash("VfxShapeLegacy"));
        assert!(matches!(
            shape.properties[&hash("emitOffset")].value,
            PropertyValue::Vector3([4.0, 5.0, 6.0])
        ));
        assert!(matches!(
            shape.properties[&hash("unknown")].value,
            PropertyValue::U32(42)
        ));
        assert_eq!(repair_emitter(&mut emitter), 0);
    }

    #[test]
    fn repairs_partial_conversion_without_overwriting_modern_position() {
        let mut emitter = structure(
            "VfxEmitterDefinitionData",
            vec![
                prop(
                    "SpawnShape",
                    PropertyValue::Struct(structure(
                        "VfxShapeLegacy",
                        vec![
                            prop("birthTranslation", PropertyValue::Vector3([1.0; 3])),
                            prop("emitOffset", PropertyValue::Vector3([2.0; 3])),
                        ],
                    )),
                ),
                prop("EmitterPosition", PropertyValue::Vector3([9.0; 3])),
            ],
        );
        assert_eq!(repair_emitter(&mut emitter), 1);
        assert!(matches!(
            emitter.properties[&hash("EmitterPosition")].value,
            PropertyValue::Vector3([9.0, 9.0, 9.0])
        ));
        assert_eq!(repair_emitter(&mut emitter), 0);
    }
}
