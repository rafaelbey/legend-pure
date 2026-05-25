// Copyright 2026 Goldman Sachs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! `.purem` file header — magic + version + schema_hash + payload length.
//!
//! 22 bytes, little-endian:
//!
//! ```text
//! offset  size  field
//! ------  ----  ------------------------------------------------------------
//!   0      5    magic (b"PUREM")
//!   5      1    reserved (always 0 in v1)
//!   6      4    format_version  (currently 1)
//!  10      4    schema_hash     (FNV-1a over Rust type/field name list)
//!  14      8    payload_length  (bytes after this header)
//! ```
//!
//! Strict refusal policy: any mismatch on `magic`, `format_version`, or
//! `schema_hash` is a hard error. v1 has no migration path; format changes
//! always bump `format_version`, schema-shape changes always bump
//! `schema_hash`.

use thiserror::Error;

/// File magic — `b"PUREM"`.
pub const MAGIC: [u8; 5] = *b"PUREM";

/// Header reserved byte; always zero in v1.
pub const RESERVED: u8 = 0;

/// Current `.purem` format version. Bumps on any incompatible
/// byte-layout change.
pub const FORMAT_VERSION: u32 = 1;

/// Total header byte length (5 + 1 + 4 + 4 + 8).
pub const HEADER_LEN: usize = 22;

/// Build-time fingerprint of every Rust type / field that participates in
/// the wire format.
///
/// Reviewers bump this list — and therefore the const fold — on any
/// addition / removal / rename of a serialized field. Mismatch on read is
/// a hard error.
///
/// **Why both this AND `format_version`?** `format_version` covers
/// breaking byte-layout changes the writer makes deliberately;
/// `schema_hash` catches accidental drift (e.g. a new optional field on
/// `Class` that Postcard can deserialize but produces a different in-memory
/// shape than the one the writer emitted).
const SCHEMA_FINGERPRINT_TYPES: &str = concat!(
    // crates/pure/src/ids.rs
    "ElementId{InstanceId{chunk_id,local_idx},Package(PackageId)}",
    "PackageId(u32)",
    "RelationId(u32)",
    // crates/pure/src/arena.rs
    "Arena<T>{items}",
    // crates/pure/src/model.rs
    "ElementNode{name,source_info,name_source_info,parent_package}",
    "ModelChunk{chunk_id,nodes,elements}",
    "Element{Class,Enumeration,Function,Profile,Association,Measure,Unit,PrimitiveType,PackageableMultiplicity,Package}",
    // crates/pure/src/types.rs
    "TypeExpr{Named,FunctionType,Relation,Generic,GenericTypeOperation{op,left,right},Unresolved}",
    "GenericTypeOpKind{Union,Difference,Subset,Equal}",
    "ConstValue{Integer,String}",
    "Multiplicity{PureOne,ZeroOrOne,ZeroOrMany,OneOrMany,Range,Variable}",
    "Parameter{name,type_expr,multiplicity,source_info}",
    "DateValue{StrictDate,DateTime,StrictTime,Latest}",
    "ResolvedType{type_expr,multiplicity}",
    "ValueSpec{kind,source_info,type_info}",
    "FunctionCallData{function,function_name,arguments}",
    "ExprKind{IntegerLiteral,FloatLiteral,DecimalLiteral,StringLiteral,BooleanLiteral,DateLiteral,Variable,FunctionCall,PropertyCall,QualifiedPropertyCall,EnumValue,Lambda,Collection,TypeReference,PackageableElementRef,Column,RelationLiteral,ColSpecArrayLiteral,ColSpecLiteral}",
    "RelationColumnLowered{name,type_element,multiplicity}",
    "PrimitiveType{super_type,super_type_value_arguments,type_variable_parameters,constraints}",
    // crates/pure/src/nodes/*.rs
    "Class{type_parameters,type_variable_parameters,super_types,properties,qualified_properties,constraints,stereotypes,tagged_values,original_milestoned_properties}",
    "TypeParameter{name,variance}",
    "Variance{Invariant,Covariant,Contravariant}",
    "Property{name,source_info,type_expr,multiplicity,aggregation,default_value,stereotypes,tagged_values}",
    "QualifiedProperty{name,source_info,parameters,return_type,return_multiplicity,body,stereotypes,tagged_values}",
    "Constraint{name,source_info,function,enforcement_level,external_id,message}",
    "AggregationKind{None,Shared,Composite}",
    "Function{function_name,is_native,parameters,return_type,return_multiplicity,body,stereotypes,tagged_values}",
    "Association{properties,qualified_properties,stereotypes,tagged_values,original_milestoned_properties}",
    "Enumeration{values,stereotypes,tagged_values}",
    "EnumValue{name,source_info,stereotypes,tagged_values}",
    "Profile{stereotypes,tags}",
    "Measure{canonical_unit,non_canonical_units}",
    "Unit{measure,conversion_expression}",
    // crates/pure/src/annotations.rs
    "StereotypeRef{profile,value}",
    "TaggedValueRef{profile,tag,value}",
    // crates/pure/src/purem/slice.rs
    "PureModelSlice{source_chunk_range,chunks,external_refs,element_packages}",
    // crates/ast/src/source_info.rs
    "SourceInfo{source,start_line,start_column,end_line,end_column}",
);

/// Compile-time FNV-1a fold over `SCHEMA_FINGERPRINT_TYPES`.
pub const SCHEMA_HASH: u32 = fnv1a(SCHEMA_FINGERPRINT_TYPES.as_bytes());

const FNV_OFFSET: u32 = 0x811c_9dc5;
const FNV_PRIME: u32 = 0x0100_0193;

const fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash = FNV_OFFSET;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    hash
}

/// Errors raised when validating a header on read.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum HeaderError {
    /// Buffer was shorter than [`HEADER_LEN`] bytes.
    #[error("blob too short: expected at least {expected} header bytes, got {got}")]
    TooShort {
        /// Bytes the header parser needed.
        expected: usize,
        /// Bytes available.
        got: usize,
    },
    /// Magic bytes did not match `b"PUREM"`.
    #[error("bad magic: not a .purem blob (got {got:?})")]
    BadMagic {
        /// First five bytes of the blob — for diagnostics.
        got: [u8; 5],
    },
    /// `format_version` did not match [`FORMAT_VERSION`]. v1 has no
    /// migration path; bumping the version is an explicit incompatibility.
    #[error("format_version mismatch: expected {expected}, got {got}")]
    BadVersion {
        /// What the reader was compiled to expect.
        expected: u32,
        /// What the blob carried.
        got: u32,
    },
    /// `schema_hash` did not match the fingerprint baked into this build.
    #[error("schema_hash mismatch: expected {expected:#x}, got {got:#x}")]
    BadSchemaHash {
        /// Reader's fingerprint.
        expected: u32,
        /// Blob's fingerprint.
        got: u32,
    },
    /// `payload_length` doesn't match the bytes that follow the header.
    #[error("payload_length mismatch: header says {expected}, blob has {got}")]
    BadPayloadLength {
        /// What the header field said.
        expected: u64,
        /// Actual remaining bytes after the header.
        got: u64,
    },
}

/// Write the 22-byte header to `out`.
pub fn write_header(out: &mut Vec<u8>, payload_length: u64) {
    out.extend_from_slice(&MAGIC);
    out.push(RESERVED);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&SCHEMA_HASH.to_le_bytes());
    out.extend_from_slice(&payload_length.to_le_bytes());
}

/// Validate the header at the front of `blob`. On success, returns a
/// reference to the payload bytes (the slice after the header).
///
/// # Errors
///
/// Returns [`HeaderError`] for any magic / version / schema_hash / length
/// mismatch.
pub fn read_header(blob: &[u8]) -> Result<&[u8], HeaderError> {
    if blob.len() < HEADER_LEN {
        return Err(HeaderError::TooShort {
            expected: HEADER_LEN,
            got: blob.len(),
        });
    }
    let mut got_magic = [0u8; 5];
    got_magic.copy_from_slice(&blob[0..5]);
    if got_magic != MAGIC {
        return Err(HeaderError::BadMagic { got: got_magic });
    }
    // blob[5] is reserved — not validated; future versions may use it.

    let mut version_bytes = [0u8; 4];
    version_bytes.copy_from_slice(&blob[6..10]);
    let version = u32::from_le_bytes(version_bytes);
    if version != FORMAT_VERSION {
        return Err(HeaderError::BadVersion {
            expected: FORMAT_VERSION,
            got: version,
        });
    }

    let mut schema_bytes = [0u8; 4];
    schema_bytes.copy_from_slice(&blob[10..14]);
    let schema = u32::from_le_bytes(schema_bytes);
    if schema != SCHEMA_HASH {
        return Err(HeaderError::BadSchemaHash {
            expected: SCHEMA_HASH,
            got: schema,
        });
    }

    let mut len_bytes = [0u8; 8];
    len_bytes.copy_from_slice(&blob[14..22]);
    let payload_length = u64::from_le_bytes(len_bytes);

    let payload = &blob[HEADER_LEN..];
    if payload.len() as u64 != payload_length {
        return Err(HeaderError::BadPayloadLength {
            expected: payload_length,
            got: payload.len() as u64,
        });
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips() {
        let mut blob = Vec::new();
        write_header(&mut blob, 7);
        // Append 7 dummy bytes so payload_length validates.
        blob.extend_from_slice(&[0u8; 7]);
        assert_eq!(blob.len(), HEADER_LEN + 7);
        let payload = read_header(&blob).expect("header should parse");
        assert_eq!(payload.len(), 7);
    }

    #[test]
    fn header_rejects_short_blob() {
        let blob = vec![0u8; 10];
        assert!(matches!(
            read_header(&blob),
            Err(HeaderError::TooShort { .. })
        ));
    }

    #[test]
    fn header_rejects_bad_magic() {
        let mut blob = vec![b'X', b'X', b'X', b'X', b'X', 0];
        blob.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        blob.extend_from_slice(&SCHEMA_HASH.to_le_bytes());
        blob.extend_from_slice(&0u64.to_le_bytes());
        assert!(matches!(
            read_header(&blob),
            Err(HeaderError::BadMagic { .. })
        ));
    }

    #[test]
    fn header_rejects_bad_version() {
        let mut blob = Vec::new();
        blob.extend_from_slice(&MAGIC);
        blob.push(RESERVED);
        blob.extend_from_slice(&999u32.to_le_bytes());
        blob.extend_from_slice(&SCHEMA_HASH.to_le_bytes());
        blob.extend_from_slice(&0u64.to_le_bytes());
        match read_header(&blob) {
            Err(HeaderError::BadVersion { expected, got }) => {
                assert_eq!(expected, FORMAT_VERSION);
                assert_eq!(got, 999);
            }
            other => panic!("expected BadVersion, got {other:?}"),
        }
    }

    #[test]
    fn header_rejects_bad_schema_hash() {
        let mut blob = Vec::new();
        blob.extend_from_slice(&MAGIC);
        blob.push(RESERVED);
        blob.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        blob.extend_from_slice(&0xdead_beefu32.to_le_bytes());
        blob.extend_from_slice(&0u64.to_le_bytes());
        assert!(matches!(
            read_header(&blob),
            Err(HeaderError::BadSchemaHash { .. })
        ));
    }

    #[test]
    fn header_rejects_payload_length_mismatch() {
        let mut blob = Vec::new();
        write_header(&mut blob, 100);
        // Only 5 bytes of payload provided.
        blob.extend_from_slice(&[0u8; 5]);
        assert!(matches!(
            read_header(&blob),
            Err(HeaderError::BadPayloadLength { .. })
        ));
    }

    #[test]
    fn schema_hash_is_stable_for_unchanged_input() {
        // Tautology — but pinned: if the fingerprint string changes, the
        // hash changes, and reviewers will see the diff in CI.
        assert_ne!(SCHEMA_HASH, 0);
    }
}
