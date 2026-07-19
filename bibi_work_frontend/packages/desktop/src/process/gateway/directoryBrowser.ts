/**
 * Bounded local directory projection used by the desktop gateway.
 *
 * Directory entries are inspected concurrently, but with a fixed ceiling so a
 * folder containing many symlinks cannot exhaust the Electron main process's
 * file descriptors.
 */
import fs from 'node:fs';
import path from 'node:path';

export type DirectoryBrowseItem = {
  name: string;
  path: string;
  isDirectory: boolean;
  isFile?: boolean;
};

export type DirectoryBrowseData = {
  items: DirectoryBrowseItem[];
  canGoUp: boolean;
  parentPath?: string;
};

export type DirectoryBrowserOptions = {
  homePath: string;
  platform?: NodeJS.Platform;
  statConcurrency?: number;
};

async function mapWithConcurrency<T, R>(
  values: readonly T[],
  concurrency: number,
  mapper: (value: T) => Promise<R>
): Promise<R[]> {
  const results = Array.from({ length: values.length }) as R[];
  let nextIndex = 0;

  async function worker(): Promise<void> {
    const index = nextIndex++;
    if (index >= values.length) return;
    results[index] = await mapper(values[index]!);
    await worker();
  }

  const workerCount = Math.min(Math.max(1, concurrency), values.length);
  await Promise.all(Array.from({ length: workerCount }, () => worker()));
  return results;
}

function listWindowsDrives(): DirectoryBrowseData {
  const items = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'
    .split('')
    .map((letter) => `${letter}:\\`)
    .filter((drivePath) => fs.existsSync(drivePath))
    .map((drivePath) => ({
      name: drivePath,
      path: drivePath,
      isDirectory: true,
      isFile: false,
    }));
  return { items, canGoUp: false };
}

async function projectDirectoryEntry(dirPath: string, dirent: fs.Dirent): Promise<DirectoryBrowseItem | null> {
  const itemPath = path.join(dirPath, dirent.name);
  if (dirent.isDirectory()) {
    return { name: dirent.name, path: itemPath, isDirectory: true, isFile: false };
  }
  if (dirent.isFile()) {
    return { name: dirent.name, path: itemPath, isDirectory: false, isFile: true };
  }
  if (!dirent.isSymbolicLink()) return null;

  try {
    const stats = await fs.promises.stat(itemPath);
    if (stats.isDirectory()) {
      return { name: dirent.name, path: itemPath, isDirectory: true, isFile: false };
    }
    if (stats.isFile()) {
      return { name: dirent.name, path: itemPath, isDirectory: false, isFile: true };
    }
  } catch {
    // Broken or inaccessible symlinks are not useful browse targets.
  }
  return null;
}

export async function browseLocalDirectory(
  rawPath: string,
  showFiles: boolean,
  options: DirectoryBrowserOptions
): Promise<DirectoryBrowseData> {
  const platform = options.platform ?? process.platform;
  if (!rawPath && platform === 'win32') {
    return listWindowsDrives();
  }

  const dirPath = path.resolve(rawPath || options.homePath);
  const stats = await fs.promises.stat(dirPath);
  if (!stats.isDirectory()) {
    throw new Error('path is not a directory');
  }

  const entries = await fs.promises.readdir(dirPath, { withFileTypes: true });
  const resolvedItems = await mapWithConcurrency(entries, options.statConcurrency ?? 32, (entry) =>
    projectDirectoryEntry(dirPath, entry)
  );
  const items = resolvedItems.filter(
    (item): item is DirectoryBrowseItem => item !== null && (showFiles || !item.isFile)
  );
  items.sort((left, right) => {
    if (left.isDirectory !== right.isDirectory) return left.isDirectory ? -1 : 1;
    return left.name.localeCompare(right.name, undefined, { sensitivity: 'base' });
  });

  const parentPath = path.dirname(dirPath);
  const isRoot = parentPath === dirPath;
  return {
    items,
    canGoUp: !isRoot || platform === 'win32',
    parentPath: isRoot ? (platform === 'win32' ? '__ROOT__' : undefined) : parentPath,
  };
}
