//! The messages the driver uses, with the field numbers of `pkg/libgv/gvproto/*.proto` in
//! mautrix-gvoice (AGPL-3.0): `threads.proto`, `account.proto`, `requests.proto`,
//! `responses.proto` and `webchannel.proto`. Only the fields the driver reads or writes are listed;
//! a field left out is skipped when decoding and not sent when encoding. Requests list every field
//! up to the highest number they use, so the pblite arrays have the reference's length.

use crate::wire::{Kind, Schema, many, one};
use Kind::{Bool, Bytes, Float, Int, Message, Str};

// threads.proto

pub static CONTACT: Schema = Schema {
    fields: &[one(1, Str), one(2, Str), one(7, Bool)],
};

pub static TRANSCRIPT_TOKEN: Schema = Schema {
    fields: &[one(1, Bytes), one(4, Float)],
};

pub static TRANSCRIPT: Schema = Schema {
    fields: &[one(1, Float), many(2, Message(&TRANSCRIPT_TOKEN))],
};

pub static ATTACHMENT_METADATA: Schema = Schema {
    fields: &[one(1, Int), one(2, Int), one(3, Int)],
};

pub static ATTACHMENT: Schema = Schema {
    fields: &[
        one(1, Str),
        one(2, Str),
        one(3, Int),
        many(4, Message(&ATTACHMENT_METADATA)),
        one(5, Int),
    ],
};

pub static MMS: Schema = Schema {
    fields: &[
        one(1, Str),
        one(2, Str),
        many(3, Message(&ATTACHMENT)),
        many(4, Message(&CONTACT)),
        one(5, Str),
        many(6, Str),
    ],
};

pub static MESSAGE: Schema = Schema {
    fields: &[
        one(1, Str),
        one(2, Int),
        one(3, Str),
        one(4, Message(&CONTACT)),
        one(5, Int),
        one(6, Int),
        one(7, Message(&TRANSCRIPT)),
        one(9, Float),
        one(10, Str),
        one(12, Str),
        one(13, Int),
        one(14, Int),
        one(15, Message(&MMS)),
        one(16, Str),
        one(17, Bool),
        one(18, Int),
    ],
};

pub static THREAD: Schema = Schema {
    fields: &[
        one(1, Str),
        one(2, Bool),
        many(3, Message(&MESSAGE)),
        one(4, Str),
        many(5, Message(&CONTACT)),
        many(6, Int),
        one(8, Bool),
        many(9, Str),
        many(12, Message(&CONTACT)),
    ],
};

pub static THREAD_ATTRIBUTES: Schema = Schema {
    fields: &[
        one(1, Str),
        one(2, Bool),
        one(3, Bool),
        one(4, Bool),
        many(5, Int),
        one(6, Bool),
        one(7, Str),
        one(8, Int),
    ],
};

// account.proto

pub static PHONE_NUMBER: Schema = Schema {
    fields: &[one(1, Str)],
};

pub static ACCOUNT_PHONE: Schema = Schema {
    fields: &[
        one(1, Message(&PHONE_NUMBER)),
        one(2, Bool),
        one(3, Int),
        one(6, Bool),
    ],
};

pub static PHONE: Schema = Schema {
    fields: &[one(1, Message(&ACCOUNT_PHONE))],
};

pub static ACCOUNT: Schema = Schema {
    fields: &[one(1, Str), many(3, Message(&PHONE))],
};

/// The branding value of a Google Voice number in `AccountPhone.branding`.
pub const BRANDING_GOOGLE_VOICE: i64 = 1;

// requests.proto

pub static REQ_GET_ACCOUNT: Schema = Schema {
    fields: &[one(2, Int)],
};

pub static WRAPPED_TXN_ID: Schema = Schema {
    fields: &[one(1, Int)],
};

pub static TRACKING_DATA: Schema = Schema {
    fields: &[one(1, Str)],
};

pub static SEND_MEDIA: Schema = Schema {
    fields: &[one(1, Int), one(2, Bytes), one(4, Str)],
};

pub static REQ_SEND_SMS: Schema = Schema {
    fields: &[
        one(5, Str),
        one(6, Str),
        many(7, Str),
        one(9, Message(&WRAPPED_TXN_ID)),
        one(10, Message(&SEND_MEDIA)),
        one(11, Message(&TRACKING_DATA)),
    ],
};

pub static UNKNOWN_WRAPPER: Schema = Schema {
    fields: &[one(2, Int), one(3, Int)],
};

pub static REQ_LIST_THREADS: Schema = Schema {
    fields: &[
        one(1, Int),
        one(2, Int),
        one(3, Int),
        one(4, Str),
        one(5, Str),
        one(6, Message(&UNKNOWN_WRAPPER)),
    ],
};

pub static REQ_GET_THREAD: Schema = Schema {
    fields: &[
        one(1, Str),
        one(2, Int),
        one(3, Str),
        one(4, Message(&UNKNOWN_WRAPPER)),
    ],
};

pub static REQ_UPDATE_ATTRIBUTES: Schema = Schema {
    fields: &[
        one(1, Message(&THREAD_ATTRIBUTES)),
        one(2, Message(&THREAD_ATTRIBUTES)),
        one(3, Int),
    ],
};

// responses.proto

pub static RESP_GET_THREAD: Schema = Schema {
    fields: &[one(1, Message(&THREAD))],
};

pub static RESP_LIST_THREADS: Schema = Schema {
    fields: &[many(1, Message(&THREAD)), one(2, Str), one(3, Str)],
};

pub static RESP_SEND_SMS: Schema = Schema {
    fields: &[one(2, Str), one(3, Str), one(4, Int), many(5, Int)],
};

pub static RESP_GET_ACCOUNT: Schema = Schema {
    fields: &[one(1, Message(&ACCOUNT))],
};

// webchannel.proto

pub static RESP_CHOOSE_SERVER: Schema = Schema {
    fields: &[one(1, Str), one(2, Int)],
};

pub static WEBCHANNEL_SESSION: Schema = Schema {
    fields: &[
        one(1, Str),
        one(2, Str),
        one(3, Str),
        one(4, Int),
        one(5, Int),
        one(6, Int),
    ],
};

pub static WEBCHANNEL_SESSION_DATA: Schema = Schema {
    fields: &[one(1, Int), one(2, Message(&WEBCHANNEL_SESSION))],
};

pub static RESP_CREATE_CHANNEL: Schema = Schema {
    fields: &[one(1, Message(&WEBCHANNEL_SESSION_DATA))],
};

pub static SUB1: Schema = Schema {
    fields: &[one(1, Str)],
};

pub static NESTED_DATA: Schema = Schema {
    fields: &[
        one(1, Str),
        one(2, Str),
        one(3, Str),
        one(4, Str),
        one(5, Str),
        one(6, Str),
        one(7, Str),
        one(9, Str),
    ],
};

pub static SUB2_DATA: Schema = Schema {
    fields: &[
        one(2, Bytes),
        one(3, Str),
        one(4, Str),
        one(5, Message(&NESTED_DATA)),
    ],
};

pub static SUB2: Schema = Schema {
    fields: &[many(1, Message(&SUB2_DATA))],
};

pub static SUB3: Schema = Schema {
    fields: &[one(1, Str)],
};

pub static WEBCHANNEL_EVENT_BODY: Schema = Schema {
    fields: &[
        one(1, Message(&SUB1)),
        one(2, Message(&SUB2)),
        one(3, Message(&SUB3)),
    ],
};

pub static WEBCHANNEL_DATA: Schema = Schema {
    fields: &[one(1, Str), one(2, Message(&WEBCHANNEL_EVENT_BODY))],
};

pub static WEBCHANNEL_ALT_DATA: Schema = Schema {
    fields: &[one(1, Bool)],
};

pub static WEBCHANNEL_DATA_WRAPPER: Schema = Schema {
    fields: &[
        many(1, Message(&WEBCHANNEL_DATA)),
        one(2, Message(&WEBCHANNEL_ALT_DATA)),
    ],
};

pub static WEBCHANNEL_EVENT: Schema = Schema {
    fields: &[one(1, Int), many(2, Message(&WEBCHANNEL_DATA_WRAPPER))],
};

pub static WEBCHANNEL_NOOP: Schema = Schema {
    fields: &[one(1, Int), many(2, Str)],
};
