# Design 12: server boosts

Members who host a relay for a server earn it recognition and higher call ceilings. A boost is never
paid for, and it never gates anything basic. At level 0, chat, files, standard calls and screen
sharing work as they always have. A boost only raises ceilings.

## Hosts

A **host** of a server is a member root that has published at least one relay for that server, where
this app has verified the relay as working within the last 72 hours.

- **Publishing.** A member announces the relays they host for a server with a signed control event in
  that server: `RelayHostSet { relays }`, at most four https URLs. The latest one by HLC per author
  wins, and an empty list withdraws. It is sealed like the other control payloads and appended. An
  older client rejects it as forged and ignores it. It is separate from `SetRelays`, which is the
  admins' list of relays the server *uses*. Hosting a relay doesn't make it a server relay, and a
  server relay doesn't make its operator a host.
- **Verifying.** Each app probes the published relays for itself, one relay per 30 s maintenance
  tick (`Engine::check_relay_hosts`). Each relay takes a turn: one never probed goes first, then the
  one probed longest ago. A relay probed in the last hour (`REPROBE_MS`) is skipped.
  - A relay passes only when a ping goes through it and nothing else (`Node::ping_via_relay`). That
    is a short-lived iroh endpoint with this device's key, a relay map holding only that relay, no
    IP transport and no address lookup. It pings a member device known to be on the relay, the
    host's own devices first, since a host's app usually picks its own relay as home.
  - The ping completes only if the relay admits this member's key and carries its packets. So
    "reachable, TLS valid, and relays for this member" is proven by traffic, not by a latency
    measurement. A net report can measure a relay that refuses the key.
  - The relay keeps the device's main connection as an inactive duplicate while the probe runs, and
    makes it active again when the probe closes (iroh-relay 1.4.1). Meanwhile, packets for this
    device on that relay reach the probe endpoint, which drops them. Nothing is lost for good:
    - QUIC retransmits on open connections, which carry gossip, sync, RPC and calls, so their data
      arrives once the main connection is back.
    - A new dial during the probe is retried by QUIC's handshake backoff and completes after it. The
      test `relay_probe::a_dial_during_a_probe_still_arrives` dials during a 3 s probe and arrives
      relayed, about 3.5 s after the probe ends.
    - Should a gossip delivery still fail, anti-entropy sync on the next tick (30 s) fetches the
      event. Call media datagrams sent in that window are lost, as on any brief network blip.
    - The window is one ping through the relay, capped at 3 s (`PROBE_WAIT`), at most once per tick
      and once an hour per relay.
  - A relay with no member device known to be on it isn't probed, and doesn't count.
  - With "Use relays offered by my servers" off, the app probes nothing and counts no host.
  - A pass is kept with its time, in memory. After a restart, a host counts again once its relay
    passes a probe. Checks and probe times of relays that no current host publishes are dropped on
    the next tick, so these maps never outgrow the published relays.
  - The probe runs on its own short-lived endpoint, so a relay is not added to the member's node
    before it passes. Once it passes, the node adds it, and drops it again once it stops counting
    or is withdrawn. The node's added relays are reconciled (`Engine::reconcile_relays`), each
    tick and on every `SetRelays`, to the wanted set, capped at 30 with the configured relays
    counted: relays the communities name, then the person's own hosted relays, then passing hosts,
    then an invite's or a community record's relays for 10 minutes. The rest are removed, so a
    long-lived device never fills its relay list. Removal waits while a call runs. The host's
    own app adds the relays it publishes, so members can reach the host through them.
  - **No private addresses.** Publishing refuses a relay whose host is a private, loopback,
    link-local or local name (`dissent_events::public_relay_host`): `192.168.1.1`, `localhost`,
    `nas.local` and the like. The same rule applies to the relays a server names. Replay still
    accepts any https URL, so released apps and older events keep their state. Each member checks
    the text again before it adds or probes a relay. Before probing, it also checks that every
    address the name resolves to is public, and before adding, that no address it resolves to now is
    private. So a host can't make members dial into their own networks. A node that runs on
    loopback alone (tests, `DISSENT_LOOPBACK=1`) accepts loopback relays.
- **Decay.** A relay that hasn't passed for more than 72 hours (`HOST_GRACE_MS`) stops counting. A
  host counts while any of their relays counts.
- **One per root.** A host counts once, however many relays they publish. Devices of one root
  publish under that root. A rotated key moves the host with the root.
- **No central authority.** There is no global count. Each app's level is what that app can verify,
  from its own network vantage point. Two members may briefly disagree, for example while one is
  offline, or when a relay is reachable from one network and not another. Ceilings apply to what each
  app sends itself, so a disagreement never breaks a call. It only means one sender may use a little
  more than another.

## Levels

The thresholds live in one table, `boosts::LEVEL_HOSTS`:

| Level | Verified hosts |
|---|---|
| 0 | 0 |
| 1 | 1 |
| 2 | 3 |
| 3 | 5 or more |

## Ceilings

One table, `boosts::CEILINGS`, indexed by level:

| Level | Voice bitrate cap | Camera | Screen share | Mesh before forwarding |
|---|---|---|---|---|
| 0 | 96 kbps | 720p30 | 1080p15 | 6 |
| 1 | 96 kbps | 1080p30 | 1080p30 | 6 |
| 2 | 128 kbps | 1080p30 | 1080p30 | 8 |
| 3 | 256 kbps | 1080p60 | 1080p60 | 10 |

Level 0 is exactly what the app allowed before boosts. Voice could already be set to 96 kbps, and the
screen ladder already reached 1080p15. So no level ever lowers a ceiling.

- **Voice.** Level 0 keeps 96 kbps, the most a voice channel could be set to before boosts, so no
  server loses anything. A voice channel's bitrate setting may be up to 256 kbps in replay. An older
  client rejects a setting above 96 as malformed and keeps the old value. When a call starts,
  this app sends at the lower of the channel's setting and its level's cap. The settings page offers
  values up to the cap.
- **Video.** The sender's adaptation ladder skips rungs above the level's frame size and frame rate.
  A rung may be missing, as when the ladder has no 1080p60 rung. Then the ceiling has no effect until
  one is added. Calls own the ladder.
- **Mesh.** A call stays a mesh up to the level's limit, then is forwarded. Forwarded calls already
  run (`04-calls.md`). `TierPolicy` takes the level's limit, but only while every device in the
  call has sent `CallMsg::Features` and has the upload for the bigger mesh. Otherwise it uses level
  0's (forwarded at 7, back at 5). See `13-forwarded-calls.md`, phase 1.

## Recognition

- **Relay host badge.** Shown on the profile card and in the member list of a server the person hosts
  for, with the servers they host for (those this app shares with them and has verified).
- **Level.** Shown next to the server name in the sidebar header, and on the invite card as
  "Level 2 · 3 relay hosts". The answering member states the level in its preview, as it does the
  member count, through a new appended `Request::InvitePreviewMore`. Its answer adds a `PreviewMore`
  (the boost, and the banner as it is) to the released `SignedPreviewFull`. A member from before it
  answers with an error, and the joiner falls back to `InvitePreviewFull` and shows no level. The
  released full preview now carries an animated banner as its still poster, so released clients
  still decode it. The card is unverified until the joiner is in.
- **Server Settings, Boosts.** Lists the current hosts and their relays, the level, what the next
  level needs, and a thank-you line.

## Privacy

Publishing a relay tells the server's members its URL, and that this member operates it. The members
learn the host's relay address, and through it, possibly where it runs. Verification only connects to
the relay as an ordinary client, so the host's relay sees each member's device connect, as
`06-threat-model.md` ("Relay for my servers") already describes. Non-members see nothing, since the
event is sealed.
