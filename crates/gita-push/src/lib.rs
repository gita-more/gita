//! What a push does after the fact.
//!
//! A push that lands on a repository has two consequences the rest of
//! the system cares about: a `push` event, and — if the pushed commit
//! carries a `.gita-ci.yml` — one or more CI runs.
//!
//! Both consequences used to belong to the HTTP transport alone. The
//! SSH transport ran the same `git receive-pack`, updated the same
//! refs, and then published nothing: a repository pushed over HTTP ran
//! its CI, the same repository pushed over SSH did not, and the only
//! visible difference was the transport the user happened to pick.
//!
//! This crate is the shared half. Both transports call
//! [`publish_push`] with the ref updates that were applied, and it does
//! the rest.
//!
//! # Which ref updates count
//!
//! A client *claims* to update some refs; the repository may refuse.
//! `git receive-pack` reports the outcome in its `report-status`
//! section, and both transports therefore only report updates that
//! actually took effect:
//!
//! * HTTP parses the commands out of the request body and then checks
//!   each one against the repository — a ref counts as updated only if
//!   it now points at the new sha.
//! * SSH parses the `report-status` the server wrote back.
//!
//! Either way, a rejected push publishes nothing and triggers no CI.

pub mod publish;
pub mod refs;

pub use publish::publish_push;
pub use refs::{parse_receive_pack_commands, ref_update_applied, RefUpdate};

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt_line(s: &str) -> Vec<u8> {
        let mut line = s.as_bytes().to_vec();
        line.push(b'\n');
        let len = line.len() + 4;
        let mut out = format!("{len:04x}").into_bytes();
        out.extend_from_slice(&line);
        out
    }

    #[test]
    fn commands_are_parsed_into_ref_updates() {
        let mut buf = pkt_line(
            "1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 refs/heads/main",
        );
        buf.extend(pkt_line(
            "3333333333333333333333333333333333333333 4444444444444444444444444444444444444444 refs/heads/other",
        ));
        buf.extend(b"0000");

        let updates = parse_receive_pack_commands(&buf);
        assert_eq!(updates.len(), 2, "got {updates:?}");
        assert_eq!(updates[0].ref_name, "refs/heads/main");
        assert_eq!(
            updates[0].old_sha,
            "1111111111111111111111111111111111111111"
        );
        assert_eq!(
            updates[0].new_sha,
            "2222222222222222222222222222222222222222"
        );
        assert_eq!(updates[1].ref_name, "refs/heads/other");
    }

    #[test]
    fn a_capabilities_section_is_stripped() {
        // The first command carries the client's capabilities after a NUL.
        let mut line =
            b"1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 refs/heads/main".to_vec();
        line.push(0);
        line.extend_from_slice(b"report-status report-status-v2 side-band-64k");
        line.push(b'\n');
        let mut buf = format!("{:04x}", line.len() + 4).into_bytes();
        buf.extend_from_slice(&line);
        buf.extend(b"0000");

        let updates = parse_receive_pack_commands(&buf);
        assert_eq!(updates.len(), 1, "got {updates:?}");
        assert_eq!(updates[0].ref_name, "refs/heads/main");
    }

    #[test]
    fn a_deletion_is_recognised() {
        let zeros = "0".repeat(40);
        let update = RefUpdate {
            old_sha: "1".repeat(40),
            new_sha: zeros.clone(),
            ref_name: "refs/heads/gone".to_string(),
        };
        assert!(update.is_deletion());

        let creation = RefUpdate {
            old_sha: zeros,
            new_sha: "2".repeat(40),
            ref_name: "refs/heads/new".to_string(),
        };
        assert!(!creation.is_deletion());
    }

    #[test]
    fn garbage_does_not_panic() {
        assert!(parse_receive_pack_commands(b"").is_empty());
        assert!(parse_receive_pack_commands(b"not a pkt line at all").is_empty());
        assert!(parse_receive_pack_commands(&[0xff, 0xfe, 0xfd]).is_empty());
    }
}

#[cfg(test)]
mod pkt_line_tests {
    use super::*;

    fn pkt_line(s: &str) -> Vec<u8> {
        let mut line = s.as_bytes().to_vec();
        line.push(b'\n');
        let len = line.len() + 4;
        let mut out = format!("{len:04x}").into_bytes();
        out.extend_from_slice(&line);
        out
    }

    const CMD: &str = "1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 refs/heads/main";

    /// A packet whose declared length is shorter than the length header
    /// itself is malformed; the parser must stop rather than read a
    /// length of its own header as a payload.
    #[test]
    fn a_length_below_the_header_size_stops_the_parse() {
        // "0002" claims a two-byte packet, which cannot hold a header.
        let body = b"0002ab";
        assert!(parse_receive_pack_commands(body).is_empty());
    }

    /// A packet that claims more bytes than the body holds is
    /// truncated; the parser must not read past the end.
    #[test]
    fn a_truncated_packet_stops_the_parse() {
        let mut body = format!("{:04x}", CMD.len() + 5).into_bytes();
        body.extend_from_slice( CMD.as_bytes());
        assert!(parse_receive_pack_commands(&body).is_empty());
    }

    /// The length is four ASCII hex digits; anything else is garbage.
    #[test]
    fn a_non_hex_length_stops_the_parse() {
        let mut body = b"zzzz".to_vec();
        body.extend_from_slice(CMD.as_bytes());
        assert!(parse_receive_pack_commands(&body).is_empty());
    }

    /// A command list without a terminating flush packet still parses:
    /// the commands arrived, only the end marker is missing.
    #[test]
    fn a_command_list_without_a_flush_packet_still_parses() {
        let body = pkt_line(CMD);
        let updates = parse_receive_pack_commands(&body);
        assert_eq!(updates.len(), 1, "got {updates:?}");
        assert_eq!(updates[0].ref_name, "refs/heads/main");
    }

    /// The command section ends at the first flush packet; anything
    /// after it (the pack data) is not a command.
    #[test]
    fn only_the_commands_before_the_flush_packet_are_parsed() {
        let mut body = pkt_line(CMD);
        body.extend(b"0000");
        body.extend(pkt_line(
            "3333333333333333333333333333333333333333 4444444444444444444444444444444444444444 refs/heads/after",
        ));
        let updates = parse_receive_pack_commands(&body);
        assert_eq!(updates.len(), 1, "got {updates:?}");
        assert_eq!(updates[0].ref_name, "refs/heads/main");
    }

    /// A line that does not carry the three fields is not a command and
    /// is skipped without ending the parse.
    #[test]
    fn a_malformed_command_is_skipped() {
        let mut body = pkt_line("garbage");
        body.extend(pkt_line(CMD));
        body.extend(b"0000");
        let updates = parse_receive_pack_commands(&body);
        assert_eq!(updates.len(), 1, "got {updates:?}");
        assert_eq!(updates[0].ref_name, "refs/heads/main");
    }

    /// The leading flush packet a client sends before the commands is
    /// consumed, not mistaken for a command.
    #[test]
    fn a_leading_flush_packet_is_consumed() {
        let mut body = b"0000".to_vec();
        body.extend(pkt_line(CMD));
        body.extend(b"0000");
        let updates = parse_receive_pack_commands(&body);
        assert_eq!(updates.len(), 1, "got {updates:?}");
        assert_eq!(updates[0].ref_name, "refs/heads/main");
    }
}
