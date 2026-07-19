/**
 * @vitest-environment node
 */
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { browseLocalDirectory } from '@process/gateway/directoryBrowser';

const temporaryDirectories: string[] = [];

async function temporaryDirectory(): Promise<string> {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-directory-browser-'));
  temporaryDirectories.push(directory);
  return directory;
}

afterEach(async () => {
  await Promise.all(
    temporaryDirectories.splice(0).map((directory) => fs.rm(directory, { recursive: true, force: true }))
  );
});

describe('browseLocalDirectory', () => {
  it('sorts directories before files and can hide files', async () => {
    const root = await temporaryDirectory();
    await fs.mkdir(path.join(root, 'z-directory'));
    await fs.mkdir(path.join(root, 'a-directory'));
    await fs.writeFile(path.join(root, 'a-file.txt'), 'a');

    const all = await browseLocalDirectory(root, true, { homePath: root, platform: 'linux' });
    expect(all.items.map((item) => [item.name, item.isDirectory])).toEqual([
      ['a-directory', true],
      ['z-directory', true],
      ['a-file.txt', false],
    ]);

    const directories = await browseLocalDirectory(root, false, { homePath: root, platform: 'linux' });
    expect(directories.items.map((item) => item.name)).toEqual(['a-directory', 'z-directory']);
  });

  it('projects valid symlinks and ignores broken symlinks', async () => {
    const root = await temporaryDirectory();
    await fs.mkdir(path.join(root, 'target-directory'));
    await fs.writeFile(path.join(root, 'target-file.txt'), 'content');
    await fs.symlink('target-directory', path.join(root, 'directory-link'));
    await fs.symlink('target-file.txt', path.join(root, 'file-link'));
    await fs.symlink('missing', path.join(root, 'broken-link'));

    const result = await browseLocalDirectory(root, true, {
      homePath: root,
      platform: 'linux',
      statConcurrency: 2,
    });

    expect(result.items.find((item) => item.name === 'directory-link')).toMatchObject({ isDirectory: true });
    expect(result.items.find((item) => item.name === 'file-link')).toMatchObject({ isFile: true });
    expect(result.items.some((item) => item.name === 'broken-link')).toBe(false);
  });

  it('uses the configured home directory for an empty path', async () => {
    const root = await temporaryDirectory();
    await fs.mkdir(path.join(root, 'child'));

    const result = await browseLocalDirectory('', false, { homePath: root, platform: 'linux' });

    expect(result.items.map((item) => item.name)).toEqual(['child']);
    expect(result.parentPath).toBe(path.dirname(root));
  });
});
