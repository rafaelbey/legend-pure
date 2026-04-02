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

//! Protocol annotation types — stereotypes and tagged values.
//!
//! Maps to Java `org.finos.legend.engine.protocol.pure.m3.extension.*`.

use serde::{Deserialize, Serialize};

use super::source_info::SourceInformation;

/// A pointer to a stereotype within a profile.
///
/// Maps to Java `org.finos.legend.engine.protocol.pure.m3.extension.StereotypePtr`.
///
/// JSON example:
/// ```json
/// {
///   "profile": "meta::pure::profiles::temporal",
///   "value": "businesstemporal",
///   "sourceInformation": { ... },
///   "profileSourceInformation": { ... }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StereotypePtr {
    /// The fully qualified profile path.
    pub profile: String,
    /// The stereotype name within the profile.
    pub value: String,
    /// Source location of the stereotype reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
    /// Source location of the profile reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_source_information: Option<SourceInformation>,
}

/// A pointer to a tag within a profile.
///
/// Maps to Java `org.finos.legend.engine.protocol.pure.m3.extension.TagPtr`.
/// Structurally identical to [`StereotypePtr`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagPtr {
    /// The fully qualified profile path.
    pub profile: String,
    /// The tag name within the profile.
    pub value: String,
    /// Source location of the tag reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
    /// Source location of the profile reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_source_information: Option<SourceInformation>,
}

/// A tagged value: a tag pointer plus a string value.
///
/// Maps to Java `org.finos.legend.engine.protocol.pure.m3.extension.TaggedValue`.
///
/// JSON example:
/// ```json
/// {
///   "tag": { "profile": "meta::pure::profiles::doc", "value": "description" },
///   "value": "A person entity"
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaggedValue {
    /// The tag pointer (profile + tag name).
    pub tag: TagPtr,
    /// The string value associated with this tag.
    pub value: String,
    /// Source location of the tagged value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src() -> SourceInformation {
        SourceInformation {
            source_id: "test.pure".into(),
            start_line: 1, start_column: 1, end_line: 1, end_column: 20,
        }
    }

    #[test]
    fn test_stereotype_ptr_serialization() {
        let s = StereotypePtr {
            profile: "meta::pure::profiles::temporal".into(),
            value: "businesstemporal".into(),
            source_information: Some(src()),
            profile_source_information: None,
        };
        let json = serde_json::to_value(&s).unwrap();

        assert_eq!(json["profile"], "meta::pure::profiles::temporal");
        assert_eq!(json["value"], "businesstemporal");
        assert!(json.get("sourceInformation").is_some());
        assert!(json.get("profileSourceInformation").is_none(),
            "None should be omitted");
    }

    #[test]
    fn test_tag_ptr_serialization() {
        let t = TagPtr {
            profile: "meta::pure::profiles::doc".into(),
            value: "description".into(),
            source_information: None,
            profile_source_information: None,
        };
        let json = serde_json::to_value(&t).unwrap();

        assert_eq!(json["profile"], "meta::pure::profiles::doc");
        assert_eq!(json["value"], "description");
        // Both source infos omitted
        let obj = json.as_object().unwrap();
        assert_eq!(obj.len(), 2);
    }

    #[test]
    fn test_tagged_value_serialization() {
        let tv = TaggedValue {
            tag: TagPtr {
                profile: "meta::pure::profiles::doc".into(),
                value: "description".into(),
                source_information: None,
                profile_source_information: None,
            },
            value: "A person entity".into(),
            source_information: None,
        };
        let json = serde_json::to_value(&tv).unwrap();

        assert_eq!(json["value"], "A person entity");
        assert_eq!(json["tag"]["profile"], "meta::pure::profiles::doc");
        assert_eq!(json["tag"]["value"], "description");
    }

    #[test]
    fn test_roundtrip_stereotype_ptr() {
        let s = StereotypePtr {
            profile: "meta::pure::profiles::temporal".into(),
            value: "businesstemporal".into(),
            source_information: Some(src()),
            profile_source_information: Some(src()),
        };
        let json_str = serde_json::to_string(&s).unwrap();
        let back: StereotypePtr = serde_json::from_str(&json_str).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn test_roundtrip_tagged_value() {
        let tv = TaggedValue {
            tag: TagPtr {
                profile: "doc".into(),
                value: "description".into(),
                source_information: None,
                profile_source_information: None,
            },
            value: "some text".into(),
            source_information: Some(src()),
        };
        let json_str = serde_json::to_string(&tv).unwrap();
        let back: TaggedValue = serde_json::from_str(&json_str).unwrap();
        assert_eq!(back, tv);
    }
}
