export type Access = "read-only" | "read-write";
/** One exclusive managed transport. A fresh object is required after finish. */
export interface Storage {
  usable(): boolean;
  access(): Access;
  capacity(): { bytes: number };
  readRange(offset: bigint, length: number): Promise<Uint8Array>;
  writeRange(offset: bigint, bytes: Uint8Array): Promise<void>;
  sync(): Promise<void>;
  close(): Promise<void>;
  /** Must abort outstanding requests without borrowing an active WASM call. */
  abortPending?(): Promise<void>;
  /** ManagedStorageSession provides this retained WebUSB grant for cancellation. */
  usbDevice?(): { close(): Promise<void> };
}
export interface Entry {
  name: string;
  kind: "file" | "directory" | "symlink" | "other";
  size: number;
  /** Ext4-only inode incarnation, read with the dependency checksum checks. */
  inode?: number;
  generation?: number;
}
export interface Inspection {
  kind: "fat" | "ext4";
  writable: boolean;
  deviceBytes: number;
  allocationUnit: number;
  totalUnits: number;
  freeBytes: number;
  dirty?: boolean;
  reservedBytes?: number;
  freeInodes?: number;
  uuid?: number[];
  features?: { compat: number; incompat: number; roCompat: number };
  needsRecovery?: boolean;
  orphanHead?: number;
  checkedWriteFormatSupported?: boolean;
  writeBlocker?: string | null;
  journal?: {
    start: number;
    sequence: number;
    error: number;
    compat: number;
    incompat: number;
    roCompat: number;
  } | null;
}
export interface FilesystemSession {
  usable(): boolean;
  access(): Access;
  inspect(): Promise<Inspection>;
  stat(path: string): Promise<Entry>;
  list(path?: string): Promise<Entry[]>;
  read(
    path: string,
    offset?: bigint | number,
    length?: number,
  ): Promise<Uint8Array>;
  createFile(path: string): Promise<void>;
  write(
    path: string,
    offset: bigint | number,
    bytes: Uint8Array,
  ): Promise<void>;
  truncate(path: string, length: bigint | number): Promise<void>;
  mkdir(path: string): Promise<void>;
  remove(path: string): Promise<void>;
  rename(source: string, destination: string): Promise<void>;
  finish(): Promise<void>;
  abort(): Promise<void>;
}
export function openFilesystem(options: {
  storage: Storage;
  kind: "fat" | "ext4";
  access?: Access;
  workerUrl?: string | URL;
  moduleUrl?: string | URL;
  mailboxBytes?: number;
  ioTimeout?: number;
  operationTimeout?: number;
}): Promise<FilesystemSession>;
