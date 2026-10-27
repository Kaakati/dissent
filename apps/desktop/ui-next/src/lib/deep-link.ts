// Links that open Dissent: dissent://invite/…, dissent://contact/…, dissent://message/… and dissent://link/…. The backend
// queues each one (src-tauri deep_link.rs) and sends it here. A link only chooses what to show. Joining, adding a contact
// and approving a device happen only when the person presses the button in the dialog. A message link moves the view.
import { refOfLink } from '@/components/chat/message-link';
import { openMessage } from '@/components/chat/jump';
import { openContactDialog, openInviteDialog } from '@/components/dialogs/deep-link-dialog';
import { parseAddressInput } from './address';
import { useShell } from './store';
import { toast } from './toasts';

const NOT_OPENABLE = "That link isn't one Dissent can open.";
const MESSAGE_MISSING = "You can't see that message on this device.";

// Routes one link by its start. Anything else is refused with a message and nothing else.
export function handleDeepLink(link: string): void {
  if (/^dissent:\/\/invite\//i.test(link)) {
    openInviteDialog(link);
    return;
  }
  if (/^dissent:\/\/contact\//i.test(link)) {
    // The address behind the link, read by the same parser the add-contact dialogs use.
    let id: string | null;
    try {
      const found = parseAddressInput(link);
      if (found && found.kind !== 'user') throw new Error(`That is a ${found.kind} address. Ask for a person's contact link.`);
      id = found?.id ?? null;
    } catch (err) {
      toast(err);
      return;
    }
    if (!id) {
      toast(NOT_OPENABLE);
      return;
    }
    openContactDialog(link, id);
    return;
  }
  if (/^dissent:\/\/message\//i.test(link)) {
    const ref = refOfLink(link);
    if (!ref) {
      toast(NOT_OPENABLE);
      return;
    }
    void openMessage(ref, MESSAGE_MISSING);
    return;
  }
  if (/^dissent:\/\/link\//i.test(link)) {
    // The code goes into the box on Settings › Devices. Pressing Approve there is still the person's choice.
    useShell.setState({ pendingDeviceLink: link });
    useShell.getState().openSettings('devices');
    return;
  }
  toast(NOT_OPENABLE);
}

export function handleDeepLinks(links: string[]): void {
  for (const link of links) handleDeepLink(link);
}
