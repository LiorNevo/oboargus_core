//! Validation for screen observations.
//!
//! The browser masks secrets before upload; the server cannot inspect pixels, so
//! it verifies the declared element map and mask manifest. A secret element that
//! is not covered by a mask rectangle is rejected.

use serde::Deserialize;

use crate::morpheus::error::{ErrorCode, MorpheusError};

/// One masked rectangle.
#[derive(Debug, Clone, Deserialize)]
pub struct MaskRegion {
    /// Element reference, when the region covers an element.
    #[serde(default, rename = "ref")]
    pub element_ref: Option<String>,
    /// Reason (`password`, `credential`, `sensitive`, `self`, `ignored`, `hidden`).
    pub reason: String,
    /// Left coordinate.
    pub x: i32,
    /// Top coordinate.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

/// One interactive element reported with an observation.
#[derive(Debug, Clone, Deserialize)]
pub struct ElementRef {
    /// Observation-local reference.
    #[serde(rename = "ref")]
    pub element_ref: String,
    /// Accessible role.
    pub role: String,
    /// Accessible name.
    #[serde(default)]
    pub name: String,
    /// `empty`, `filled`, `masked`, `checked`, `unchecked`, or `selected`.
    #[serde(default)]
    pub value_state: String,
}

/// Deployment limits for an observation.
#[derive(Debug, Clone, Copy)]
pub struct ObservationLimits {
    /// Maximum long-edge dimension.
    pub max_dimension: i32,
    /// Maximum JPEG bytes.
    pub max_bytes: i32,
    /// Maximum reported elements.
    pub max_elements: usize,
    /// Maximum mask rectangles.
    pub max_masks: usize,
}

/// Validates an uploaded observation against the deployment limits.
pub fn validate_observation(
    width: i32,
    height: i32,
    image_mime: &str,
    image_bytes: i32,
    masks: &[MaskRegion],
    elements: &[ElementRef],
    limits: ObservationLimits,
) -> Result<(), MorpheusError> {
    if width <= 0 || height <= 0 || width.max(height) > limits.max_dimension {
        return Err(reject("image dimensions are outside the allowed range"));
    }
    if image_mime != "image/jpeg" {
        return Err(reject("only image/jpeg observations are accepted"));
    }
    if image_bytes <= 0 || image_bytes > limits.max_bytes {
        return Err(reject("image size is outside the allowed range"));
    }
    if elements.len() > limits.max_elements {
        return Err(reject("too many elements in the observation"));
    }
    if masks.len() > limits.max_masks {
        return Err(reject("too many mask regions in the observation"));
    }
    for region in masks {
        if region.w <= 0 || region.h <= 0 {
            return Err(reject("mask region has no area"));
        }
        if region.x < 0
            || region.y < 0
            || region.x.saturating_add(region.w) > width
            || region.y.saturating_add(region.h) > height
        {
            return Err(reject("mask region lies outside the image"));
        }
    }
    for element in elements {
        if element.value_state == "masked"
            && !masks.iter().any(|mask| {
                mask.element_ref
                    .as_deref()
                    .is_some_and(|value| value == element.element_ref)
            })
        {
            return Err(reject(&format!(
                "secret element {} is not masked",
                element.element_ref
            )));
        }
    }
    Ok(())
}

fn reject(message: &str) -> MorpheusError {
    MorpheusError::tool(ErrorCode::ObservationRejected, message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> ObservationLimits {
        ObservationLimits {
            max_dimension: 1280,
            max_bytes: 524_288,
            max_elements: 200,
            max_masks: 200,
        }
    }

    fn element(state: &str) -> ElementRef {
        ElementRef {
            element_ref: "e1".to_string(),
            role: "textbox".to_string(),
            name: "API key".to_string(),
            value_state: state.to_string(),
        }
    }

    #[test]
    fn rejects_an_unmasked_secret() {
        let result = validate_observation(
            800,
            600,
            "image/jpeg",
            1000,
            &[],
            &[element("masked")],
            limits(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn accepts_a_masked_secret() {
        let mask = MaskRegion {
            element_ref: Some("e1".to_string()),
            reason: "credential".to_string(),
            x: 0,
            y: 0,
            w: 100,
            h: 20,
        };
        let result = validate_observation(
            800,
            600,
            "image/jpeg",
            1000,
            &[mask],
            &[element("masked")],
            limits(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_oversized_images() {
        assert!(validate_observation(4000, 600, "image/jpeg", 1000, &[], &[], limits()).is_err());
        assert!(validate_observation(800, 600, "image/png", 1000, &[], &[], limits()).is_err());
    }
}
