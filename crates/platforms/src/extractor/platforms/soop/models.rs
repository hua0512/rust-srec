use serde::{Deserialize, Deserializer};

#[derive(Deserialize, Debug)]
pub struct SoopPlayerResponse {
    #[serde(rename = "CHANNEL")]
    pub channel: SoopChannel,
}

#[derive(Deserialize, Debug, Default)]
pub struct SoopChannel {
    #[serde(rename = "RESULT", deserialize_with = "deserialize_int_or_string")]
    pub result: i64,
    #[serde(rename = "BNO", default)]
    pub bno: Option<String>,
    #[serde(rename = "BJID", default)]
    pub bjid: Option<String>,
    #[serde(rename = "BJNICK", default)]
    pub bjnick: Option<String>,
    #[serde(rename = "TITLE", default)]
    pub title: Option<String>,
    #[serde(rename = "CATE", default)]
    pub cate: Option<String>,
    #[serde(rename = "CATEGORY_TAGS", default)]
    pub category_tags: Option<Vec<String>>,
    #[serde(rename = "RMD", default)]
    pub rmd: Option<String>,
    #[serde(rename = "CDN", default)]
    pub cdn: Option<String>,
    #[serde(rename = "BPWD", default)]
    pub bpwd: Option<String>,
    #[serde(rename = "AID", default)]
    pub aid: Option<String>,
    #[serde(rename = "VIEWPRESET", default)]
    pub viewpreset: Option<Vec<SoopViewPreset>>,
    /// Chat host (preferred over CHIP).
    #[serde(rename = "CHDOMAIN", default)]
    pub chdomain: Option<String>,
    /// Chat IP fallback when CHDOMAIN is empty.
    #[serde(rename = "CHIP", default)]
    pub chip: Option<String>,
    /// Base chat port; WSS uses CHPT + 1.
    #[serde(
        rename = "CHPT",
        default,
        deserialize_with = "deserialize_opt_int_or_string"
    )]
    pub chpt: Option<i64>,
    /// Chat room number used in SVC_JOINCH.
    #[serde(rename = "CHATNO", default)]
    pub chatno: Option<String>,
    /// Fan/session ticket required on chat join.
    #[serde(rename = "FTK", default)]
    pub ftk: Option<String>,
    /// Present (true) on geo/GDPR stub payloads that also use RESULT=0.
    #[serde(rename = "GDPR", default)]
    pub gdpr: Option<bool>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SoopViewPreset {
    pub label: String,
    pub name: String,
}

#[derive(Deserialize, Debug)]
pub struct SoopStreamAssign {
    pub view_url: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct SoopLoginResponse {
    #[serde(rename = "RESULT", deserialize_with = "deserialize_int_or_string")]
    pub result: i64,
}

#[derive(Deserialize, Debug)]
pub struct SoopStationStatus {
    #[serde(rename = "DATA")]
    pub data: SoopStationData,
}

#[derive(Deserialize, Debug)]
pub struct SoopStationData {
    pub user_nick: String,
}

fn deserialize_int_or_string<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum IntOrString {
        Int(i64),
        String(String),
    }

    match IntOrString::deserialize(deserializer)? {
        IntOrString::Int(value) => Ok(value),
        IntOrString::String(value) => value
            .trim()
            .parse::<i64>()
            .map_err(serde::de::Error::custom),
    }
}

fn deserialize_opt_int_or_string<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OptIntOrString {
        Null,
        Int(i64),
        String(String),
    }

    match Option::<OptIntOrString>::deserialize(deserializer)? {
        None | Some(OptIntOrString::Null) => Ok(None),
        Some(OptIntOrString::Int(value)) => Ok(Some(value)),
        Some(OptIntOrString::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                return Ok(None);
            }
            value
                .parse::<i64>()
                .map(Some)
                .map_err(serde::de::Error::custom)
        }
    }
}
