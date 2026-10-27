//! Wire order guard for `Payload`. See the note in `rpc.rs`'s `wire_order_tests`: the declaration order of a
//! serde enum is its wire tag, so the order pinned here must not change.

// Wire order guard. serde's derive tags each variant of these enums by its position in the enum, and postcard
// writes that tag. So the declaration order is the wire format, and a change to it is a wire change.
//
// The lists pin the order as of v0.1.11. To add a variant, APPEND it at the END of the enum. Never insert,
// reorder, rename or remove one: every later tag would shift, and an older peer or an older stored event would
// decode the wrong variant. A variant added after v0.1.11 needs no entry here. If a variant must change shape,
// bump the wire version rather than editing these lists.

const V0111_PAYLOAD: &[&str] = &[
    "ChannelCreate",
    "VoiceChannelCreate",
    "Message",
    "Edit",
    "Delete",
    "RoleGrant",
    "RoleRevoke",
    "Ban",
    "Unban",
    "HandleClaim",
    "Join",
    "Leave",
    "KeyRotation",
    "DeviceRevoke",
    "Sealed",
    "Reply",
    "React",
    "Unreact",
    "SetRelays",
    "JoinAsStorage",
    "SecretRotation",
    "SealedControl",
    "ChannelUpdate",
    "ChannelDelete",
    "InviteCreate",
    "InviteRevoke",
    "JoinVia",
    "ProfileUpdate",
    "CommunityIconSet",
    "MemberSealedControl",
    "MemberOnlyControl",
    "HistoryVisibility",
    "CommunityRename",
    "CommunityBannerSet",
    "CommunityProfileSet",
    "ChannelAccess",
    "PrivateSealed",
    "ProfileBannerSet",
    "ChannelCreatePrivate",
];

/// The variant names of `enum_name` in declaration order, read from `source`. See the note above.
fn variant_names(source: &str, enum_name: &str) -> Vec<String> {
    let header = format!("enum {enum_name} {{");
    let mut names = Vec::new();
    let mut depth = 0i32;
    let mut inside = false;
    for line in source.lines() {
        let code = line.trim_start();
        if !inside {
            inside = line.contains(&header);
            if inside {
                depth = 1;
            }
            continue;
        }
        // A variant is at depth 1, indented by four spaces, and starts with a capital letter.
        if depth == 1
            && line.starts_with("    ")
            && !line.starts_with("     ")
            && code.starts_with(|c: char| c.is_ascii_uppercase())
        {
            names.push(code.chars().take_while(|c| c.is_alphanumeric()).collect());
        }
        if !code.starts_with("//") {
            depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        }
        if depth == 0 {
            break;
        }
    }
    names
}

#[test]
fn payload_order_matches_v0111() {
    let names = variant_names(include_str!("event.rs"), "Payload");
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(names[..V0111_PAYLOAD.len()], *V0111_PAYLOAD, "Payload order changed: see the note above");
}
