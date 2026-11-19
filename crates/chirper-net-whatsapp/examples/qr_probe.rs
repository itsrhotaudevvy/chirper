//! Live probe: connect to the WhatsApp WebSocket with fresh in-memory keys, complete the Noise
//! handshake, and print the shape of the first server node. Prints tag names only: never refs,
//! keys or JIDs.

use chirper_net_whatsapp::binary::Node;
use chirper_net_whatsapp::keys::DeviceKeys;
use chirper_net_whatsapp::payload;
use chirper_net_whatsapp::socket::{Connection, WS_URL};
use chirper_net_whatsapp::version;
use prost::Message;

fn print_tags(node: &Node, depth: usize) {
    println!("{}{}", "  ".repeat(depth), node.tag);
    for child in node.child_nodes() {
        print_tags(child, depth + 1);
    }
}

fn ref_count(node: &Node) -> usize {
    let own = if node.tag == "ref" { 1 } else { 0 };
    own + node.child_nodes().iter().map(ref_count).sum::<usize>()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let keys = DeviceKeys::generate()?;
    let (primary, secondary, revision) = version::fetch(&chirper_bridge::http_client()).await?;
    println!("client version {primary}.{secondary}.{revision}");
    let payload = payload::registration(&keys, (primary, secondary, revision))?.encode_to_vec();
    let mut connection = Connection::connect(WS_URL, keys.noise.clone(), payload).await?;
    println!("handshake ok, server cert verified");

    let node = connection.next_node().await?;
    println!("first server node:");
    print_tags(&node, 1);
    let refs = ref_count(&node);
    println!("qr ref count {refs}");
    connection.close().await;
    Ok(())
}
