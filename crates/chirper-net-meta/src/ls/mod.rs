//! The lightspeed response format: the JSON `payload` of an `/ls_resp` or task answer, interpreted
//! into the typed tables of [`table`]. See [`decode`] for the step interpreter.

pub mod decode;
pub mod table;

use std::collections::HashSet;

use chirper_bridge::Error;
use serde::Deserialize;
use serde_json::Value as Json;

pub use table::LsTable;

/// The `payload` string of a publish response: `{"name": ..., "step": ...}`.
#[derive(Debug, Deserialize)]
struct LightSpeedData {
    #[serde(default)]
    step: Json,
}

/// Interpret the payload of one response. `sp` lists the dependency names the server sent with it;
/// only those procedures that also have a typed row are read, as the reference does.
pub fn parse_payload(payload: &str, sp: &[String]) -> Result<LsTable, Error> {
    let data: LightSpeedData = serde_json::from_str(payload)
        .map_err(|err| Error::Protocol(format!("lightspeed payload is not JSON: {err}")))?;
    let typed = table::typed_procedures();
    let wanted: HashSet<String> = sp
        .iter()
        .filter(|name| typed.contains(name.as_str()))
        .cloned()
        .collect();
    let calls = decode::Decoder::new(Some(wanted)).run(&data.step);
    Ok(LsTable::from_calls(&calls))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sp(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_thread_and_message_response_decodes_into_rows() {
        let payload = json!({
            "name": "respond",
            "step": [1,
                [5, "deleteThenInsertThread",
                    [19, "1700000000000"], [19, "0"], "hello there", "Ada & Co",
                    "", false, [19, "3"], [19, "17"], [19, "5"], [19, "2"],
                    "", "", [19, "0"], [19, "0"], [19, "0"], [19, "0"], false,
                    [19, "0"], [9], [9], [19, "0"], [19, "0"], [19, "0"], [19, "0"]
                ],
                [5, "insertMessage",
                    "hi", "", [19, "0"], [19, "146"], [9], [19, "1700000000001"], [19, "1"],
                    [19, "2"], "mid.$abc", "otid-1", [19, "42"], [19, "0"], false, [19, "0"],
                    [9], [19, "1"], [19, "0"], false, [19, "0"], "", "", "", "", "", [19, "0"]
                ]
            ]
        })
        .to_string();
        let table = parse_payload(
            &payload,
            &sp(&["deleteThenInsertThread", "insertMessage"]),
        )
        .expect("payload parses");
        assert_eq!(table.threads.len(), 1);
        assert_eq!(table.threads[0].thread_key, 17);
        assert_eq!(table.threads[0].thread_name, "Ada & Co");
        assert_eq!(table.threads[0].thread_type, 2);
        assert_eq!(table.messages.len(), 1);
        let message = &table.messages[0];
        assert_eq!(message.text, "hi");
        assert_eq!(message.thread_key, 146);
        assert_eq!(message.message_id, "mid.$abc");
        assert_eq!(message.offline_threading_id, "otid-1");
        assert_eq!(message.sender_id, 42);
        assert_eq!(message.timestamp_ms, 1_700_000_000_001);
    }

    #[test]
    fn procedures_the_server_did_not_list_are_ignored() {
        let payload = json!({
            "step": [1, [5, "deleteThread", [19, "5"], false]]
        })
        .to_string();
        let table = parse_payload(&payload, &sp(&["insertMessage"])).expect("parses");
        assert_eq!(table, LsTable::default());
        let table = parse_payload(&payload, &sp(&["deleteThread"])).expect("parses");
        assert_eq!(table.deleted_threads, vec![5]);
    }

    #[test]
    fn a_payload_that_is_not_json_is_a_protocol_error() {
        assert!(matches!(
            parse_payload("{not json", &sp(&[])),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn sync_blocks_carry_their_cursors() {
        let payload = json!({
            "step": [1, [5, "executeFirstBlockForSyncTransactionV4",
                [19, "1"], [19, "99"], "cur-1", "cur-2", [19, "0"], [9], true,
                [19, "0"], [19, "0"], [19, "1"]]]
        })
        .to_string();
        let table = parse_payload(
            &payload,
            &sp(&["executeFirstBlockForSyncTransactionV4"]),
        )
        .expect("parses");
        assert_eq!(table.sync_blocks.len(), 1);
        assert_eq!(table.sync_blocks[0].database_id, 1);
        assert_eq!(table.sync_blocks[0].next_cursor, "cur-2");
        assert!(table.sync_blocks[0].send_sync_params);
    }
}
