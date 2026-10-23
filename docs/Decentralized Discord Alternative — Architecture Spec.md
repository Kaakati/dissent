# Decentralized Discord Alternative — Architecture Spec

Oct 24, 2026 · @Mr. Mohamad

## Goal and principles

Build a Discord-like app (communities, text channels, voice, video, usernames) in which no server is required to run it: every role can be run by anyone, and none is mandatory.

- **No mandatory operator.** Relays, seeders and call forwarders are roles any peer or volunteer can take on; the app degrades gracefully when none exist.
- **Identity is a key.** Users and communities are Ed25519 public keys; names are layered on top, never the root of trust.
- **Infrastructure sees ciphertext only.** MLS encrypts messages and SFrame encrypts media end to end; relays and forwarders carry bytes they cannot read.
- **Authority lives in the data.** Roles, bans and usernames are signed events that every peer validates itself.
- **No consensus layer.** Each community needs causal ordering, not global ordering, so there is no blockchain.
- **Rust-native stack.** iroh, OpenMLS and str0m form the core.

## Architecture by layer

Each layer has one default choice, and every infrastructure role in it can be run by anyone.

| Layer | Choice | Key details |
| --- | --- | --- |
| Identity | Ed25519 root key + signed device keys | Seed phrase or social recovery (k-of-n friends co-sign a key rotation) |
| Usernames | `name·keysuffix`, per-community `@handle`, local petnames, optional DNS-verified handle | Key suffix ≥ 40 bits; community handle is first claim in causal order, ties broken by event hash; Unicode TR39 confusable checks |
| Naming and discovery | Pkarr records on the Mainline DHT | Community address = its public key; bootstrap from seed nodes, a peer cache, mDNS and invite links with peer addresses |
| Transport | iroh (QUIC + hole punching) | Fallback to relays volunteered by public-IP peers (AutoNAT-detected) or anyone running the open-source relay binary |
| Live messaging | `iroh-gossip` (HyParView + PlumTree), one topic per channel | Gossip reaches online peers only; libp2p gossipsub is the alternative |
| Catch-up | Range-based set reconciliation (Negentropy or `iroh-docs`) | Run with a few neighbours on reconnect to fill gaps |
| Data model | Signed, hash-linked event DAG | Event = author, channel, parent hashes, hybrid logical clock timestamp, body |
| Encryption | MLS (OpenMLS) for groups | MLS exporter secret derives SFrame keys for media |
| Permissions | Roles and bans as signed DAG events | Every peer replays causal history and rejects events from authors lacking permission at that point; deterministic conflict resolution |
| Media | str0m media engine, signalling over gossip | Tiers below; SFrame end-to-end encryption; Opus FEC, DRED, NACK |
| Availability | Replication across member devices + volunteer seeder nodes | Target replication factor per community; seeder = headless client |
| Push | UnifiedPush on Android; content-blind APNs relay on iOS | The iOS relay is the one unavoidable dependency; anyone can run it |

### Media tiers

| Call size | Topology | Notes |
| --- | --- | --- |
| 1:1 | Direct P2P | Lowest latency; relay path adds about 20–80 ms |
| 2–6 video, about 15 audio | Mesh | Each peer uploads N−1 streams |
| Up to about 50 | Peer-elected SFU | Desktop on wired fibre, no battery limit; backup pre-elected, failover under about 2 s |
| Stages, 100+ | Cascaded peer SFUs | Forwarding tree; each forwarder adds one hop |

A peer SFU with 20 Mbps upload forwards about 12 video streams at 720p (about 1.5 Mbps each with simulcast or AV1 SVC). For audio, it serves 150+ listeners when it forwards only the top 3 speakers at about 32 kbps.

## Ruled out

Four options were considered and dropped because each adds a mandatory operator or a dependency the design does not need.

| Option | Why it is out |
| --- | --- |
| Blockchain | Chat needs causal ordering per community, not global consensus; a chain adds latency and fees and makes data permanent and public. It could return later only as an optional resolver for globally unique handles (ENS or Handshake). |
| Cloudflare Tunnel | Public hostnames proxy HTTP/HTTPS only (no public UDP, so no media); each owner needs a domain on Cloudflare; it adds a single provider. Pkarr gives key-based addressing without a domain. |
| LiveKit (self-hosted) | Server-signed JWT auth, a single operator per room, no cascading across operators, and a second media stack; not truly decentralized. |
| Central signalling or TURN server | Signalling travels over gossip, and peers with public IPs act as relays. |

## Build phases

Text ships first as the MVP (end of phase 2); voice and video follow once the data layer is proven.

1. **Core:** identity and device keys, iroh networking, Pkarr discovery, invite links.
   - Exit: two peers behind different NATs find each other by public key and connect, directly or via a relay.
2. **Text (MVP):** event DAG, gossip, set-reconciliation sync, MLS, community usernames, permission validation.
   - Exit: a community survives concurrent bans and edits with every peer converging on the same state.
3. **Resilience:** seeder node binary, relay binary, multi-device support, key recovery.
   - Exit: history stays available with every member device offline as long as one seeder is up.
4. **Voice and video:** 1:1 and mesh calls, then peer-SFU election with failover, simulcast/SVC, SFrame E2EE.
   - Exit: a 20-person call recovers from losing its SFU peer in under about 2 s.
5. **Polish:** impersonation defences (TR39 confusables, safety numbers, key-change warnings), push relays, DNS-verified handles.

## Risks to spike first

DAG conflict resolution is the hardest correctness problem; spike it before committing to the data model.

| Risk | Why it matters | Spike |
| --- | --- | --- |
| Concurrent permission conflicts | A ban and the banned user's post can arrive in either order; peers must converge identically | Model state resolution (Matrix-style) and fuzz it with randomised event orderings |
| Hole-punching success on mobile | Carrier CGNAT breaks direct paths, pushing traffic to relays | Measure direct-vs-relayed rates across Saudi and regional carriers |
| Peer-SFU quality | Home upload limits the call size where no strong peer is present | Load-test a str0m forwarder on typical home fibre and LTE |
| Small-community availability | With nobody online, history cannot be fetched | Test replication-factor targets and seeder onboarding friction |
| iOS push | APNs requires a relay, the one central dependency | Prototype a content-blind wake-up relay that anyone can run |

## Prior art and references

This is a reading list for the spikes, not a set of verified sources.

| Project | What to take from it |
| --- | --- |
| iroh (n0) | Dial-by-key QUIC, relays, `iroh-gossip`, `iroh-docs` |
| Pkarr | Signed DNS-style records on the Mainline DHT |
| Keet (Holepunch) | Closest working P2P Discord-like app, including calls |
| Matrix | Event DAG and state resolution for permission conflicts |
| p2panda, Willow protocol | P2P data models with capabilities and deletion |
| OpenMLS, SFrame | Group key agreement and media E2EE |
| str0m | Sans-IO Rust WebRTC media engine |
| Negentropy | Range-based set reconciliation |
| Ink & Switch local-first papers | CRDTs and offline-first design |

Sources: [Cloudflare Tunnel setup docs](https://developers.cloudflare.com/tunnel/setup), [Cloudflare Tunnel UDP limits (Localtonet)](https://localtonet.com/blog/cloudflare-tunnel-alternative)
