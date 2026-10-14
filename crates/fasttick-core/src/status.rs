//! Node status values, as defined by BehaviorTree.CPP (`BT::NodeStatus`).
//!
//! The numeric values and the text names match `basic_types.h` and
//! `basic_types.cpp` of BT.CPP 4.x. The live protocol and the log files use
//! these values on the wire.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The status of a tree node.
///
/// Serde uses the BT.CPP text names (`"IDLE"`, `"RUNNING"`, `"SUCCESS"`,
/// `"FAILURE"`, `"SKIPPED"`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[repr(u8)]
pub enum NodeStatus {
    /// The node is not active.
    #[default]
    Idle = 0,
    /// The node is active and has not finished.
    Running = 1,
    /// The node finished with success.
    Success = 2,
    /// The node finished with failure.
    Failure = 3,
    /// The node was skipped (for example by a `_skipIf` pre-condition).
    Skipped = 4,
}

impl NodeStatus {
    /// All values, in numeric order.
    pub const ALL: [NodeStatus; 5] = [
        NodeStatus::Idle,
        NodeStatus::Running,
        NodeStatus::Success,
        NodeStatus::Failure,
        NodeStatus::Skipped,
    ];

    /// Converts the numeric BT.CPP value (0 to 4) to a status.
    pub fn from_u8(value: u8) -> Option<NodeStatus> {
        match value {
            0 => Some(NodeStatus::Idle),
            1 => Some(NodeStatus::Running),
            2 => Some(NodeStatus::Success),
            3 => Some(NodeStatus::Failure),
            4 => Some(NodeStatus::Skipped),
            _ => None,
        }
    }

    /// The numeric BT.CPP value.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// The BT.CPP text name, for example `"SUCCESS"`.
    pub fn as_str(self) -> &'static str {
        match self {
            NodeStatus::Idle => "IDLE",
            NodeStatus::Running => "RUNNING",
            NodeStatus::Success => "SUCCESS",
            NodeStatus::Failure => "FAILURE",
            NodeStatus::Skipped => "SKIPPED",
        }
    }

    /// True for SUCCESS and FAILURE (BT.CPP `isStatusCompleted`).
    pub fn is_completed(self) -> bool {
        matches!(self, NodeStatus::Success | NodeStatus::Failure)
    }

    /// True for all values except IDLE and SKIPPED (BT.CPP `isStatusActive`).
    pub fn is_active(self) -> bool {
        !matches!(self, NodeStatus::Idle | NodeStatus::Skipped)
    }
}

impl fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error for a text that is not a status name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown node status: {0:?}")]
pub struct ParseStatusError(pub String);

impl FromStr for NodeStatus {
    type Err = ParseStatusError;

    /// Accepts the exact BT.CPP names (`IDLE`, `RUNNING`, `SUCCESS`,
    /// `FAILURE`, `SKIPPED`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NodeStatus::ALL
            .into_iter()
            .find(|status| status.as_str() == s)
            .ok_or_else(|| ParseStatusError(s.to_owned()))
    }
}

/// Offset that the publisher adds to the previous status when a node goes
/// back to IDLE (`IDLE_FROM_SUCCESS = 10 + SUCCESS`, and so on).
pub const IDLE_FROM_OFFSET: u8 = 10;

/// A status byte as the STATUS reply of the publisher sends it.
///
/// Values 0 to 4 are a [`NodeStatus`]. Values 11 to 14 mean "IDLE, and the
/// previous status was RUNNING, SUCCESS, FAILURE or SKIPPED". The publisher
/// writes `10 + previous` for each transition to IDLE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RawStatus(pub u8);

/// A decoded [`RawStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DecodedStatus {
    /// The status now.
    pub current: NodeStatus,
    /// The status before the node went back to IDLE, if the byte tells it.
    pub previous: Option<NodeStatus>,
}

impl RawStatus {
    /// Decodes the byte. Returns `None` for values that BT.CPP does not send.
    pub fn decode(self) -> Option<DecodedStatus> {
        if let Some(current) = NodeStatus::from_u8(self.0) {
            return Some(DecodedStatus {
                current,
                previous: None,
            });
        }
        let previous = self
            .0
            .checked_sub(IDLE_FROM_OFFSET)
            .and_then(NodeStatus::from_u8)
            .filter(|status| *status != NodeStatus::Idle)?;
        Some(DecodedStatus {
            current: NodeStatus::Idle,
            previous: Some(previous),
        })
    }

    /// Encodes a status the same way as the publisher.
    pub fn encode(current: NodeStatus, previous: NodeStatus) -> RawStatus {
        if current == NodeStatus::Idle && previous != NodeStatus::Idle {
            RawStatus(IDLE_FROM_OFFSET + previous.as_u8())
        } else {
            RawStatus(current.as_u8())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_values_match_btcpp() {
        assert_eq!(NodeStatus::Idle.as_u8(), 0);
        assert_eq!(NodeStatus::Running.as_u8(), 1);
        assert_eq!(NodeStatus::Success.as_u8(), 2);
        assert_eq!(NodeStatus::Failure.as_u8(), 3);
        assert_eq!(NodeStatus::Skipped.as_u8(), 4);
        for status in NodeStatus::ALL {
            assert_eq!(NodeStatus::from_u8(status.as_u8()), Some(status));
        }
        assert_eq!(NodeStatus::from_u8(5), None);
    }

    #[test]
    fn names_round_trip() {
        for status in NodeStatus::ALL {
            assert_eq!(status.as_str().parse::<NodeStatus>(), Ok(status));
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(json, format!("\"{}\"", status.as_str()));
            let back: NodeStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, status);
        }
        assert!("success".parse::<NodeStatus>().is_err());
    }

    #[test]
    fn raw_status_decoding() {
        assert_eq!(
            RawStatus(2).decode(),
            Some(DecodedStatus {
                current: NodeStatus::Success,
                previous: None
            })
        );
        assert_eq!(
            RawStatus(13).decode(),
            Some(DecodedStatus {
                current: NodeStatus::Idle,
                previous: Some(NodeStatus::Failure)
            })
        );
        assert_eq!(
            RawStatus(11).decode().unwrap().previous,
            Some(NodeStatus::Running)
        );
        assert_eq!(RawStatus(10).decode(), None);
        assert_eq!(
            RawStatus(14).decode().unwrap().previous,
            Some(NodeStatus::Skipped)
        );
        assert_eq!(RawStatus(15).decode(), None);
        assert_eq!(RawStatus(255).decode(), None);
    }

    #[test]
    fn raw_status_encoding() {
        assert_eq!(
            RawStatus::encode(NodeStatus::Idle, NodeStatus::Success),
            RawStatus(12)
        );
        assert_eq!(
            RawStatus::encode(NodeStatus::Idle, NodeStatus::Idle),
            RawStatus(0)
        );
        assert_eq!(
            RawStatus::encode(NodeStatus::Running, NodeStatus::Idle),
            RawStatus(1)
        );
    }

    #[test]
    fn completed_and_active() {
        assert!(NodeStatus::Success.is_completed());
        assert!(NodeStatus::Failure.is_completed());
        assert!(!NodeStatus::Running.is_completed());
        assert!(NodeStatus::Running.is_active());
        assert!(!NodeStatus::Skipped.is_active());
        assert!(!NodeStatus::Idle.is_active());
    }
}
