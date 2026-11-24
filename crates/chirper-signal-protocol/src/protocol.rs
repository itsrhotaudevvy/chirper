//! Wire message types. Only the ciphertext type enum exists so far.

/// The type of a serialized ciphertext. Values match libsignal `protocol.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiphertextMessageType {
    /// A `SignalMessage` (Double Ratchet message).
    Whisper = 2,
    /// A `PreKeySignalMessage` (session-establishing message).
    PreKey = 3,
    /// A `SenderKeyMessage`.
    SenderKey = 7,
    /// Plaintext content (no Signal ciphertext).
    Plaintext = 8,
}
