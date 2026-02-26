use coap_lite::Packet;
use coap_message::MinimalWritableMessage;
use coap_message_implementations::inmemory_write::Message;
use liboscore::raw::oscore_requestid_t;
use liboscore::PrimitiveContext;

use super::OscoreError;

pub fn protect_request(
    packet: &Packet,
    ctx: &mut PrimitiveContext,
) -> Result<(Packet, oscore_requestid_t), OscoreError> {
    // Step A: Allocate outer buffer
    // Estimate size: original wire bytes + headroom for OSCORE option + AEAD tag
    let wire_bytes = packet.to_bytes().map_err(|_| OscoreError::PacketEncodingFailed)?;
    let mut code_byte: u8 = 0;
    let mut buf = vec![0u8; wire_bytes.len() + 64];
    let mut outer_msg = Message::new(&mut code_byte, &mut buf);

    // Step B: Call liboscore::protect_request
    // The outer_msg receives the OSCORE option + encrypted ciphertext.
    // The writer closure populates the inner plaintext with code, options, payload.
    let (request_id, write_result) = liboscore::protect_request(
        &mut outer_msg,
        ctx,
        |pm| -> Result<(), OscoreError> {
            // Inner code
            pm.set_code(u8::from(packet.header.code));

            // Options in ascending order (BTreeMap::iter is sorted by key)
            for (&opt_num, values) in packet.options() {
                for value in values {
                    pm.add_option(opt_num, value)
                        .map_err(|_| OscoreError::ProtectFailed)?;
                }
            }

            // Payload
            if !packet.payload.is_empty() {
                pm.set_payload(&packet.payload)
                    .map_err(|_| OscoreError::ProtectFailed)?;
            }

            Ok(())
        },
    )
    .map_err(|_| OscoreError::ProtectFailed)?;

    write_result?;

    // Step C: Reconstruct outer Packet via wire-format round-trip
    let used = outer_msg.finish();
    let token = packet.get_token();
    let raw_header = packet.header.to_raw();

    // Build wire bytes: 4-byte header + token + outer options/payload from liboscore
    let mut wire = Vec::with_capacity(4 + token.len() + used);
    raw_header
        .serialize_into(&mut wire)
        .map_err(|_| OscoreError::PacketEncodingFailed)?;
    // Overwrite the code byte with what liboscore set (outer code for OSCORE)
    wire[1] = code_byte;
    wire.extend_from_slice(token);
    wire.extend_from_slice(&buf[..used]);

    let outer_packet =
        Packet::from_bytes(&wire).map_err(|_| OscoreError::PacketDecodingFailed)?;

    Ok((outer_packet, request_id))
}
