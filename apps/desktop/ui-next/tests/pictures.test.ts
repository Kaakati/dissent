// Unit tests for the sealed-picture URLs. Run with `npm run test:unit` (node --test, no build step).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { pictureSrc, pictureUrls } from '../src/lib/pictures.ts';

// Tauri's convertFileSrc, as it builds a URL on Windows.
(globalThis as unknown as { window: unknown }).window = {
  __TAURI__: { core: { convertFileSrc: (path: string, protocol = 'asset') => `http://${protocol}.localhost/${encodeURIComponent(path)}` } },
};

test('a local path becomes a dimg URL, and a URL stays as it is', () => {
  assert.equal(pictureSrc('C:\\data\\avatars\\a.webp'), 'http://dimg.localhost/C%3A%5Cdata%5Cavatars%5Ca.webp');
  assert.equal(pictureSrc('/home/ada/avatars/a.webp'), 'http://dimg.localhost/%2Fhome%2Fada%2Favatars%2Fa.webp');
  const url = 'http://dimg.localhost/C%3A%5Cx.png';
  assert.equal(pictureSrc(url), url, 'a converted path is not converted twice');
  assert.equal(pictureSrc('data:image/png;base64,AAAA'), 'data:image/png;base64,AAAA');
  assert.equal(pictureSrc(null), null);
});

test('a response has its picture paths converted under the picture keys only', () => {
  const view = {
    root_id: 'r',
    avatar: 'C:\\p\\avatars\\a.webp',
    banner: '/p/avatars/b.webp',
    banner_preset: 3,
    servers: [{ id: 's', icon: 'C:\\p\\avatars\\i.webp', icon_poster: null, name: '/not/a/picture' }],
    attachments: [{ local_path: 'C:\\Users\\ada\\Downloads\\photo.png', thumb: { path: 'C:\\p\\thumbnails\\t.jpg' } }],
    already: { avatar: 'http://dimg.localhost/x' },
  };
  pictureUrls(view);
  assert.equal(view.avatar, 'http://dimg.localhost/C%3A%5Cp%5Cavatars%5Ca.webp');
  assert.equal(view.banner, 'http://dimg.localhost/%2Fp%2Favatars%2Fb.webp');
  assert.equal(view.servers[0].icon, 'http://dimg.localhost/C%3A%5Cp%5Cavatars%5Ci.webp');
  assert.equal(view.servers[0].icon_poster, null);
  assert.equal(view.servers[0].name, '/not/a/picture', 'other keys are left alone');
  // Downloads load as plain files, and a thumbnail through pictureSrc where it is shown.
  assert.equal(view.attachments[0].local_path, 'C:\\Users\\ada\\Downloads\\photo.png');
  assert.equal(view.attachments[0].thumb.path, 'C:\\p\\thumbnails\\t.jpg');
  assert.equal(view.already.avatar, 'http://dimg.localhost/x');
});
