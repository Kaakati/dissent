// Renders the dialogs opened with openDialog() and the confirmation from confirm(). Mounted once, in App.
import { X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import {
  Dialog,
  DialogBody,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { answerConfirm, closeDialogById, type DialogEntry, useDialogs } from '@/lib/dialog';

function DialogFrame({ entry }: { entry: DialogEntry }) {
  const close = () => closeDialogById(entry.id);
  return (
    <Dialog open onOpenChange={(open) => { if (!open) close(); }}>
      <DialogContent
        wide={entry.wide}
        className={entry.className}
        // Radix would give the focus to the trigger, which these dialogs do not have.
        onCloseAutoFocus={(e) => {
          e.preventDefault();
          if (entry.opener?.isConnected) entry.opener.focus();
        }}
      >
        <DialogHeader>
          <div className="min-w-0">
            <DialogTitle>{entry.title}</DialogTitle>
            {entry.sub ? <DialogDescription>{entry.sub}</DialogDescription> : null}
          </div>
          <DialogClose asChild>
            <Button variant="ghost" size="icon" aria-label="Close" className="-mt-1 -mr-2 shrink-0">
              <X />
            </Button>
          </DialogClose>
        </DialogHeader>
        <DialogBody>
          {entry.content(close)}
          {entry.footer ? <DialogFooter className="mt-0">{entry.footer(close)}</DialogFooter> : null}
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
}

export function DialogHost() {
  const dialogs = useDialogs((s) => s.dialogs);
  const confirmation = useDialogs((s) => s.confirm);
  return (
    <>
      {dialogs.map((entry) => (
        <DialogFrame key={entry.id} entry={entry} />
      ))}
      {confirmation ? (
        <AlertDialog open onOpenChange={(open) => { if (!open) answerConfirm(confirmation.id, false); }}>
          <AlertDialogContent
            onCloseAutoFocus={(e) => {
              e.preventDefault();
              if (confirmation.opener?.isConnected) confirmation.opener.focus();
            }}
          >
            <AlertDialogHeader>
              <AlertDialogTitle>{confirmation.title ?? 'Please confirm'}</AlertDialogTitle>
              <AlertDialogDescription>{confirmation.message}</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter className="mt-s-5">
              <Button variant="outline" onClick={() => answerConfirm(confirmation.id, false)}>
                Cancel
              </Button>
              <Button
                variant={confirmation.danger === false ? 'default' : 'destructive'}
                onClick={() => answerConfirm(confirmation.id, true)}
              >
                {confirmation.okLabel}
              </Button>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      ) : null}
    </>
  );
}
