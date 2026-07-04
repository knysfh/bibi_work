use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::fmt;

use crate::features::core::errors::AppError;

#[derive(Debug, Deserialize, PartialEq, Serialize, Clone, Type, Copy)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    MacOS,
    AndroidPhone,
    IOS,
    IPad,
    Linux,
    AndroidTablet,
    Web,
    Unknow,
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let platform_str = match self {
            Platform::Windows => "windows",
            Platform::MacOS => "macos",
            Platform::Linux => "linux",
            Platform::AndroidPhone => "androidphone",
            Platform::IOS => "ios",
            Platform::AndroidTablet => "androidtablet",
            Platform::IPad => "ipad",
            Platform::Web => "web",
            Platform::Unknow => "unknow",
        };
        write!(f, "{}", platform_str)
    }
}

impl From<Platform> for String {
    fn from(platform: Platform) -> Self {
        platform.to_string()
    }
}

impl std::str::FromStr for Platform {
    type Err = AppError;

    fn from_str(platform_str: &str) -> Result<Self, Self::Err> {
        match platform_str.to_lowercase().as_str() {
            "windows" => Ok(Platform::Windows),
            "macos" => Ok(Platform::MacOS),
            "linux" => Ok(Platform::Linux),
            "androidphone" => Ok(Platform::AndroidPhone),
            "ios" => Ok(Platform::IOS),
            "androidtablet" => Ok(Platform::AndroidTablet),
            "ipad" => Ok(Platform::IPad),
            "web" => Ok(Platform::Web),
            _ => Ok(Platform::Unknow),
        }
    }
}

impl From<String> for Platform {
    fn from(s: String) -> Self {
        s.parse().unwrap_or(Platform::Unknow)
    }
}

impl Platform {
    pub fn is_supported(&self) -> bool {
        !matches!(self, Platform::Unknow)
    }
}
