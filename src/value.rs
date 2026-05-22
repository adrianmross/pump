use std::fmt;
use std::ops::{Index, IndexMut};

use anyhow::{Result, bail};
use indexmap::IndexMap;
use saphyr::{
    LoadableYamlNode, MarkedYamlOwned, ScalarOwned, ScalarStyle, Tag, YamlDataOwned, YamlEmitter,
    YamlOwned,
};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Sequence(Vec<Value>),
    Mapping(IndexMap<String, Value>),
    Tagged { tag: String, value: Box<Value> },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    Integer(i64),
    Unsigned(u64),
    Float(f64),
}

impl Value {
    pub fn parse_yaml_documents(input: &str) -> Result<Vec<Self>> {
        MarkedYamlOwned::load_from_str(input)?
            .iter()
            .map(Self::from_marked_yaml)
            .collect()
    }

    pub fn from_json(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(value) => Self::Bool(value),
            serde_json::Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    Self::Number(Number::Integer(value))
                } else if let Some(value) = value.as_u64() {
                    Self::Number(Number::Unsigned(value))
                } else {
                    Self::Number(Number::Float(value.as_f64().unwrap_or_default()))
                }
            }
            serde_json::Value::String(value) => Self::String(value),
            serde_json::Value::Array(values) => {
                Self::Sequence(values.into_iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(values) => Self::Mapping(
                values
                    .into_iter()
                    .map(|(key, value)| (key, Self::from_json(value)))
                    .collect(),
            ),
        }
    }

    pub fn from_marked_yaml(value: &MarkedYamlOwned) -> Result<Self> {
        match &value.data {
            YamlDataOwned::Value(value) => Ok(Self::from_scalar(value)),
            YamlDataOwned::Representation(value, _, tag) => {
                let tag = tag.as_ref().map(std::borrow::Cow::Borrowed);
                let parsed = ScalarOwned::parse_from_cow_and_metadata(
                    value.as_str().into(),
                    ScalarStyle::Plain,
                    tag.as_ref(),
                )
                .unwrap_or_else(|| ScalarOwned::String(value.clone()));
                Ok(Self::from_scalar(&parsed))
            }
            YamlDataOwned::Sequence(values) => values
                .iter()
                .map(Self::from_marked_yaml)
                .collect::<Result<Vec<_>>>()
                .map(Self::Sequence),
            YamlDataOwned::Mapping(values) => {
                let mut output = IndexMap::new();

                for (key, value) in values {
                    let key = Self::string_key_from_marked_yaml(key)?;
                    output.insert(key, Self::from_marked_yaml(value)?);
                }

                Ok(Self::Mapping(output))
            }
            YamlDataOwned::Tagged(tag, value) => Ok(Self::Tagged {
                tag: tag.to_string(),
                value: Box::new(Self::from_marked_yaml(value)?),
            }),
            YamlDataOwned::Alias(_) => bail!("YAML aliases are not supported"),
            YamlDataOwned::BadValue => bail!("invalid YAML value"),
        }
    }

    pub fn to_yaml_string(&self) -> Result<String> {
        let yaml_owned = self.to_yaml_owned();
        let yaml = saphyr::Yaml::from(&yaml_owned);
        let mut output = String::new();
        YamlEmitter::new(&mut output).dump(&yaml)?;
        Ok(output)
    }

    pub fn is_mapping(&self) -> bool {
        self.as_mapping().is_some()
    }

    pub fn as_mapping(&self) -> Option<&IndexMap<String, Value>> {
        match self {
            Self::Mapping(value) => Some(value),
            Self::Tagged { value, .. } => value.as_mapping(),
            _ => None,
        }
    }

    pub fn as_mapping_mut(&mut self) -> Option<&mut IndexMap<String, Value>> {
        match self {
            Self::Mapping(value) => Some(value),
            Self::Tagged { value, .. } => value.as_mapping_mut(),
            _ => None,
        }
    }

    pub fn is_sequence(&self) -> bool {
        self.as_sequence().is_some()
    }

    pub fn as_sequence(&self) -> Option<&[Value]> {
        match self {
            Self::Sequence(value) => Some(value),
            Self::Tagged { value, .. } => value.as_sequence(),
            _ => None,
        }
    }

    pub fn as_sequence_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Self::Sequence(value) => Some(value),
            Self::Tagged { value, .. } => value.as_sequence_mut(),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            Self::Tagged { value, .. } => value.as_str(),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_mapping().and_then(|map| map.get(key))
    }

    fn from_scalar(value: &ScalarOwned) -> Self {
        match value {
            ScalarOwned::Null => Self::Null,
            ScalarOwned::Boolean(value) => Self::Bool(*value),
            ScalarOwned::Integer(value) => Self::Number(Number::Integer(*value)),
            ScalarOwned::FloatingPoint(value) => Self::Number(Number::Float(value.into_inner())),
            ScalarOwned::String(value) => Self::String(value.clone()),
        }
    }

    fn string_key_from_marked_yaml(value: &MarkedYamlOwned) -> Result<String> {
        match Self::from_marked_yaml(value)? {
            Self::String(value) => Ok(value),
            Self::Number(Number::Integer(value)) => Ok(value.to_string()),
            Self::Number(Number::Unsigned(value)) => Ok(value.to_string()),
            Self::Number(Number::Float(value)) => Ok(format_float(value)),
            Self::Bool(value) => Ok(value.to_string()),
            Self::Null => Ok("null".to_string()),
            Self::Tagged { value, .. } => match *value {
                Self::String(value) => Ok(value),
                _ => bail!("YAML mapping keys must be scalar values"),
            },
            Self::Sequence(_) | Self::Mapping(_) => {
                bail!("YAML mapping keys must be scalar values")
            }
        }
    }

    fn to_yaml_owned(&self) -> YamlOwned {
        match self {
            Self::Null => YamlOwned::Value(ScalarOwned::Null),
            Self::Bool(value) => YamlOwned::Value(ScalarOwned::Boolean(*value)),
            Self::Number(Number::Integer(value)) => YamlOwned::Value(ScalarOwned::Integer(*value)),
            Self::Number(Number::Unsigned(value)) => {
                YamlOwned::Representation(value.to_string(), ScalarStyle::Plain, None)
            }
            Self::Number(Number::Float(value)) => {
                YamlOwned::Representation(format_float(*value), ScalarStyle::Plain, None)
            }
            Self::String(value) => YamlOwned::Value(ScalarOwned::String(value.clone())),
            Self::Sequence(values) => {
                YamlOwned::Sequence(values.iter().map(Self::to_yaml_owned).collect())
            }
            Self::Mapping(values) => {
                let mut output = saphyr::MappingOwned::new();
                for (key, value) in values {
                    output.insert(
                        YamlOwned::Value(ScalarOwned::String(key.clone())),
                        value.to_yaml_owned(),
                    );
                }
                YamlOwned::Mapping(output)
            }
            Self::Tagged { tag, value } => {
                YamlOwned::Tagged(parse_tag(tag), Box::new(value.to_yaml_owned()))
            }
        }
    }
}

impl Serialize for Value {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Null => serializer.serialize_none(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Number(Number::Integer(value)) => serializer.serialize_i64(*value),
            Self::Number(Number::Unsigned(value)) => serializer.serialize_u64(*value),
            Self::Number(Number::Float(value)) => serializer.serialize_f64(*value),
            Self::String(value) => serializer.serialize_str(value),
            Self::Sequence(values) => {
                let mut sequence = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    sequence.serialize_element(value)?;
                }
                sequence.end()
            }
            Self::Mapping(values) => {
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
            Self::Tagged { value, .. } => value.serialize(serializer),
        }
    }
}

impl Index<&str> for Value {
    type Output = Value;

    fn index(&self, index: &str) -> &Self::Output {
        self.get(index)
            .unwrap_or_else(|| panic!("key {index:?} not found"))
    }
}

impl IndexMut<&str> for Value {
    fn index_mut(&mut self, index: &str) -> &mut Self::Output {
        self.as_mapping_mut()
            .and_then(|map| map.get_mut(index))
            .unwrap_or_else(|| panic!("key {index:?} not found"))
    }
}

impl Index<usize> for Value {
    type Output = Value;

    fn index(&self, index: usize) -> &Self::Output {
        self.as_sequence()
            .and_then(|sequence| sequence.get(index))
            .unwrap_or_else(|| panic!("index {index} out of bounds"))
    }
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(formatter, "null"),
            Self::Bool(value) => write!(formatter, "{value}"),
            Self::Number(Number::Integer(value)) => write!(formatter, "{value}"),
            Self::Number(Number::Unsigned(value)) => write!(formatter, "{value}"),
            Self::Number(Number::Float(value)) => write!(formatter, "{}", format_float(*value)),
            Self::String(value) => write!(formatter, "{value}"),
            Self::Sequence(_) | Self::Mapping(_) | Self::Tagged { .. } => {
                write!(
                    formatter,
                    "{}",
                    serde_json::to_string(self).unwrap_or_default()
                )
            }
        }
    }
}

fn parse_tag(tag: &str) -> Tag {
    if let Some(suffix) = tag.strip_prefix('!') {
        Tag {
            handle: "!".to_string(),
            suffix: suffix.to_string(),
        }
    } else {
        Tag {
            handle: "!".to_string(),
            suffix: tag.to_string(),
        }
    }
}

fn format_float(value: f64) -> String {
    let value = value.to_string();
    if value.contains(['.', 'e', 'E']) {
        value
    } else {
        format!("{value}.0")
    }
}
