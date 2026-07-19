/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { execFile, spawn } from 'child_process';
import { shell } from 'electron';
import { promisify } from 'util';

const execFileAsync = promisify(execFile);
const ALLOWED_EXTERNAL_PROTOCOLS = new Set(['http:', 'https:', 'mailto:']);

export function validateExternalUrl(rawUrl: string): string {
  let url: URL;
  try {
    url = new URL(rawUrl);
  } catch {
    throw new Error('external URL is invalid');
  }
  if (!ALLOWED_EXTERNAL_PROTOCOLS.has(url.protocol)) {
    throw new Error(`external URL protocol is not allowed: ${url.protocol}`);
  }
  if ((url.protocol === 'http:' || url.protocol === 'https:') && (url.username || url.password)) {
    throw new Error('external URL credentials are not allowed');
  }
  return url.toString();
}

export async function commandExists(command: string): Promise<boolean> {
  try {
    await execFileAsync(process.platform === 'win32' ? 'where' : 'which', [command]);
    return true;
  } catch {
    return false;
  }
}

export async function checkToolInstalled(tool: string): Promise<boolean> {
  if (tool === 'vscode') return commandExists('code');
  if (tool === 'explorer') return true;
  if (tool === 'terminal') {
    if (process.platform === 'win32' || process.platform === 'darwin') return true;
    return (await commandExists('x-terminal-emulator')) || (await commandExists('gnome-terminal'));
  }
  return commandExists(tool);
}

function spawnDetached(command: string, args: string[], cwd?: string): void {
  const child = spawn(command, args, {
    cwd,
    detached: true,
    stdio: 'ignore',
    windowsHide: true,
  });
  child.on('error', (error) => {
    console.error(`[BiWork] Failed to spawn ${command}:`, error);
  });
  child.unref();
}

export async function openFolderWithTool(folderPath: string, tool: string): Promise<void> {
  if (tool === 'vscode') {
    spawnDetached('code', [folderPath]);
    return;
  }
  if (tool === 'terminal') {
    if (process.platform === 'darwin') {
      spawnDetached('open', ['-a', 'Terminal', folderPath]);
      return;
    }
    if (process.platform === 'win32') {
      spawnDetached('cmd.exe', ['/c', 'start', '', 'cmd.exe', '/K', 'cd', '/d', folderPath]);
      return;
    }
    if (await commandExists('x-terminal-emulator')) {
      spawnDetached('x-terminal-emulator', [], folderPath);
      return;
    }
    if (await commandExists('gnome-terminal')) {
      spawnDetached('gnome-terminal', ['--working-directory', folderPath]);
      return;
    }
    throw new Error('terminal is not installed');
  }
  const error = await shell.openPath(folderPath);
  if (error) throw new Error(error);
}
