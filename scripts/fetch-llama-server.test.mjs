import { describe, expect, it } from 'vitest';

import { BACKENDS, DEFAULT_BACKEND, assetsFor } from './fetch-llama-server.mjs';

/** Every platform the fetch script maps a Rust triple to. */
const PLATFORMS = [
  'linux-x64',
  'linux-arm64',
  'darwin-x64',
  'darwin-arm64',
  'win32-x64',
  'win32-arm64',
];

describe('assetsFor', () => {
  it('fetches the GPU build unless asked otherwise', () => {
    expect(DEFAULT_BACKEND).toBe('gpu');
    expect(assetsFor('linux-x64', undefined, 'b1')).toEqual([
      'llama-b1-bin-ubuntu-vulkan-x64.tar.gz',
    ]);
  });

  it('uses Vulkan on Windows and Linux, which reaches every GPU vendor', () => {
    expect(assetsFor('win32-x64', 'gpu', 'b1')).toEqual(['llama-b1-bin-win-vulkan-x64.zip']);
    expect(assetsFor('linux-arm64', 'gpu', 'b1')).toEqual([
      'llama-b1-bin-ubuntu-vulkan-arm64.tar.gz',
    ]);
  });

  it('takes Metal on Apple silicon from the build upstream already compiles it into', () => {
    expect(assetsFor('darwin-arm64', 'gpu', 'b1')).toEqual(['llama-b1-bin-macos-arm64.tar.gz']);
  });

  it('still produces an engine where there is no GPU build to have', () => {
    // An Intel Mac and Windows on Arm have nothing better upstream. The
    // default is what every installer is built with, so it cannot refuse a
    // platform Chief ships on.
    expect(assetsFor('darwin-x64', 'gpu', 'b1')).toEqual(assetsFor('darwin-x64', 'cpu', 'b1'));
    expect(assetsFor('win32-arm64', 'gpu', 'b1')).toEqual(assetsFor('win32-arm64', 'cpu', 'b1'));

    for (const platform of PLATFORMS) {
      expect(assetsFor(platform, DEFAULT_BACKEND, 'b1').length).toBeGreaterThan(0);
    }
  });

  it('keeps the CPU build Chief shipped before, by name', () => {
    expect(assetsFor('linux-x64', 'cpu', 'b1')).toEqual(['llama-b1-bin-ubuntu-x64.tar.gz']);
    expect(assetsFor('win32-x64', 'cpu', 'b1')).toEqual(['llama-b1-bin-win-cpu-x64.zip']);
  });

  it('fetches the CUDA runtime beside a CUDA engine, under its own unversioned name', () => {
    expect(assetsFor('win32-x64', 'cuda', 'b1')).toEqual([
      'llama-b1-bin-win-cuda-12.4-x64.zip',
      'cudart-llama-bin-win-cuda-12.4-x64.zip',
    ]);
  });

  it('says so where upstream publishes no CUDA build, rather than fetching something else', () => {
    expect(() => assetsFor('linux-x64', 'cuda', 'b1')).toThrow(/no cuda build for linux-x64/);
    expect(() => assetsFor('darwin-arm64', 'cuda', 'b1')).toThrow(/win32-x64/);
  });

  it('refuses a backend it does not know', () => {
    expect(BACKENDS).toEqual(['gpu', 'cpu', 'cuda']);
    expect(() => assetsFor('linux-x64', 'rocm', 'b1')).toThrow(/must be one of gpu, cpu, cuda/);
  });

  it('refuses a platform it does not know', () => {
    expect(() => assetsFor('freebsd-x64', 'cpu', 'b1')).toThrow(/no llama.cpp build/);
  });
});
