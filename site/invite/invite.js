// Hands an invite from this page to the Dissent app. The invite is the part of the address after the "#", which
// browsers never send to a server. It is checked against the short-invite format from crates/dissent-discovery
// (dss1 address, then the secret, the generation and an optional invite id) before the app is asked to open it.
(() => {
  // bech32 data characters for the address, RFC 4648 base32 for the secret and the invite id, and a decimal generation.
  const INVITE = /^dss1[qpzry9x8gf2tvdw0s3jn54khce6mua7l]{58}\.[a-z2-7]{52}\.[0-9]{1,20}(\.[a-z2-7]{26})?$/;

  const status = document.getElementById('status');
  const hint = document.getElementById('hint');
  const invalid = document.getElementById('invalid');
  const copy = document.getElementById('copy');

  const invite = location.hash.slice(1).trim().toLowerCase();

  if (INVITE.test(invite)) {
    status.textContent = 'Opening Dissent…';
    // If the app is not installed, nothing happens; the hint tells the visitor what to do.
    setTimeout(() => {
      status.textContent = 'Dissent did not open.';
      hint.hidden = false;
    }, 2500);
    copy.hidden = false;
    location.href = 'dissent://invite/' + invite;
  } else {
    status.hidden = true;
    invalid.hidden = false;
  }

  copy.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(location.href);
      copy.textContent = 'Copied';
    } catch {
      copy.textContent = 'Copy failed. Copy the address from the address bar.';
    }
  });
})();
