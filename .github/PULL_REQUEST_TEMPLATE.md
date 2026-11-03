## Summary

<!-- What does this change do, and why? Link the issue it addresses, for example "Closes #123". -->

## How it was tested

<!-- Which tests you added or ran, and any manual testing (operating systems, networks, number of devices). -->

## Checklist

- [ ] Commit subjects are imperative and start with a verb such as Add, Fix or Harden; body lines meant for the release notes start with `- `.
- [ ] `cargo test -p <crate>` passes for every crate I touched.
- [ ] `cargo clippy -p <crate> --all-targets -- -D warnings` passes for every crate I touched, including on Linux for platform-gated code.
- [ ] Networked tests I added are `#[ignore]`, stay on loopback, and pass through `scripts/fwtest.sh <package> <test-target or lib> --ignored`.
- [ ] Interface changes: `npm run test:ui` passes in `apps/desktop/ui-next`, and the mock and driver cover the new behaviour.
- [ ] New dependencies are added to the root `[workspace.dependencies]`.
- [ ] Wire enums only gained variants at the end; new event kinds go through the `SealedControl` carrier.
- [ ] New stored files are sealed, and older profiles still load.
- [ ] Security-sensitive changes update the design note and `docs/design/06-threat-model.md`.
- [ ] Documentation is updated where behaviour changed.
