// Create Category and Rename Category: one name field, at most 64 characters, Enter saves. Delete asks first, and says
// that the channels stay. Each one sets the server's layout, which the store applies at once and puts back on failure.
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { DialogFooter } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { addCategory, deleteCategory, layoutOf, MAX_CATEGORIES, MAX_CATEGORY_NAME, renameCategory, categoriesOf } from '@/lib/channel-layout';
import { confirm, openDialog } from '@/lib/dialog';
import { useShell } from '@/lib/store';
import { toast } from '@/lib/toasts';

function NameBody({ initial, action, onSave, close }: { initial: string; action: string; onSave: (name: string) => void; close: () => void }) {
  const [name, setName] = useState(initial);
  const clean = name.trim();
  const save = () => {
    if (!clean) return;
    onSave(clean);
    close();
  };
  return (
    <>
      <Input
        aria-label="Category name"
        placeholder="New category"
        maxLength={MAX_CATEGORY_NAME}
        autoFocus
        autoComplete="off"
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && !e.nativeEvent.isComposing) {
            e.preventDefault();
            save();
          }
        }}
      />
      <DialogFooter>
        <Button variant="secondary" onClick={close}>Cancel</Button>
        <Button onClick={save} disabled={!clean}>{action}</Button>
      </DialogFooter>
    </>
  );
}

const current = () => {
  const community = useShell.getState().community;
  return community ? layoutOf(community) : null;
};

export function openCreateCategoryDialog(): void {
  const entries = current();
  if (!entries) return;
  if (categoriesOf(entries).length >= MAX_CATEGORIES) {
    toast(`A server has at most ${MAX_CATEGORIES} categories`);
    return;
  }
  openDialog({
    title: 'Create Category',
    content: (close) => (
      <NameBody
        initial=""
        action="Create Category"
        close={close}
        onSave={(name) => {
          const now = current();
          if (now) void useShell.getState().setLayout(addCategory(now, name), 'Could not create the category');
        }}
      />
    ),
  });
}

export function openRenameCategoryDialog(category: string, name: string): void {
  openDialog({
    title: 'Rename Category',
    content: (close) => (
      <NameBody
        initial={name}
        action="Save"
        close={close}
        onSave={(next) => {
          const now = current();
          if (now) void useShell.getState().setLayout(renameCategory(now, category, next), 'Could not rename the category');
        }}
      />
    ),
  });
}

export async function deleteCategoryAsked(category: string, name: string): Promise<void> {
  const ok = await confirm({
    title: 'Delete Category',
    message: `Delete the category “${name}”? Its channels stay, outside any category.`,
    okLabel: 'Delete Category',
    danger: true,
  });
  const now = current();
  if (ok && now) await useShell.getState().setLayout(deleteCategory(now, category), 'Could not delete the category');
}
