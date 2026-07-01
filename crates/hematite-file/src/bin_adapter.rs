//! BIN parsing adapter using rs_bin.

use crate::convert::rs_bin_tree_to_hematite;
use anyhow::Result;
use hematite_core::traits::BinProvider;
use hematite_types::bin::BinTree;
use rs_bin::Bin as RsBin;
use rs_io::{Parse, Serialize};

/// BIN provider backed by rs_bin.
pub struct FileBinProvider;

impl FileBinProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FileBinProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl BinProvider for FileBinProvider {
    fn parse_bytes(&self, data: &[u8]) -> Result<BinTree> {
        let rs_bin = RsBin::from_bytes(data)
            .map_err(|e| anyhow::anyhow!("Failed to parse BIN: {:?}", e))?;
        rs_bin_tree_to_hematite(rs_bin)
    }

    fn write_bytes(&self, tree: &BinTree) -> Result<Vec<u8>> {
        use crate::convert::hematite_tree_to_rs_bin;

        let rs_bin = hematite_tree_to_rs_bin(tree)?;

        rs_bin
            .to_bytes()
            .map_err(|e| anyhow::anyhow!("Failed to write BIN: {:?}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::bin::{BinObject, BinProperty, PropertyValue};
    use hematite_types::hash::{FieldHash, PathHash, TypeHash};
    use indexmap::IndexMap;

    #[test]
    fn test_roundtrip_empty_tree() {
        let provider = FileBinProvider::new();
        let tree = BinTree {
            objects: IndexMap::new(),
            linked: vec![],
        };

        let bytes = provider.write_bytes(&tree).unwrap();
        let parsed = provider.parse_bytes(&bytes).unwrap();

        assert_eq!(tree.objects.len(), parsed.objects.len());
    }

    #[test]
    fn test_roundtrip_simple_object() {
        let provider = FileBinProvider::new();

        let mut properties = IndexMap::new();
        properties.insert(
            0x1111,
            BinProperty {
                name_hash: FieldHash(0x1111),
                value: PropertyValue::I32(42),
            },
        );
        properties.insert(
            0x2222,
            BinProperty {
                name_hash: FieldHash(0x2222),
                value: PropertyValue::String("test".to_string()),
            },
        );

        let obj = BinObject {
            path_hash: PathHash(0x1234),
            class_hash: TypeHash(0x5678),
            properties,
        };

        let mut objects = IndexMap::new();
        objects.insert(0x1234, obj);

        let tree = BinTree {
            objects,
            linked: vec![],
        };

        let bytes = provider.write_bytes(&tree).unwrap();
        let parsed = provider.parse_bytes(&bytes).unwrap();

        assert_eq!(tree.objects.len(), parsed.objects.len());
        let parsed_obj = parsed.objects.get(&0x1234).unwrap();
        assert_eq!(parsed_obj.path_hash.0, 0x1234);
        assert_eq!(parsed_obj.class_hash.0, 0x5678);
        assert_eq!(parsed_obj.properties.len(), 2);

        if let PropertyValue::I32(val) = parsed_obj.properties.get(&0x1111).unwrap().value {
            assert_eq!(val, 42);
        } else {
            panic!("Expected I32 value");
        }

        if let PropertyValue::String(val) = &parsed_obj.properties.get(&0x2222).unwrap().value {
            assert_eq!(val, "test");
        } else {
            panic!("Expected String value");
        }
    }
}
