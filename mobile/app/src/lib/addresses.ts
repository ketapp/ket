// A desktop's advertised and hand-added ways to reach the same paired key.

import { validRelayUrl } from '@ket/remote';

import type { SavedHost } from './store';

/** Fills the address fields in records saved before protocol 1.17. */
export function normalizeHost(host: SavedHost): SavedHost {
  return {
    ...host,
    addresses: host.addresses ?? [host.relay],
    manualAddresses: host.manualAddresses ?? [],
  };
}

/** Every usable address, including the pairing address as a fallback. */
export function dialAddresses(host: SavedHost): string[] {
  return [...new Set([...(host.addresses ?? []), ...(host.manualAddresses ?? []), host.relay])]
    .filter(validRelayUrl);
}

/** Adds an address the person supplied; invalid or duplicate entries fail. */
export function addManualAddress(host: SavedHost, address: string): SavedHost {
  const url = address.trim();
  if (!validRelayUrl(url)) throw new Error('Enter a ws:// or wss:// address with a host and optional port.');
  if (dialAddresses(host).includes(url)) throw new Error('This address is already saved.');
  return { ...host, manualAddresses: [...(host.manualAddresses ?? []), url] };
}

/** Removes only an address this phone's owner added. */
export function removeManualAddress(host: SavedHost, address: string): SavedHost {
  return { ...host, manualAddresses: (host.manualAddresses ?? []).filter((url) => url !== address) };
}

/** What a desktop's reach code says: which desktop, and every address it can
 * be reached at. Its QR code is in the desktop's Settings › Devices › Away
 * from home, for a phone paired before Tailscale or ket's relay was on. */
export interface Reach {
  hostId: string;
  addresses: string[];
}

/** The reach code in what was scanned or opened — `ket://reach?host=…&at=…`
 * — or `null` when it is not one, or names no address this phone could use. */
export function reachIn(scanned: string): Reach | null {
  let url: URL;
  try {
    url = new URL(scanned.trim());
  } catch {
    return null;
  }
  if (url.protocol !== 'ket:' || url.host !== 'reach') return null;
  return reachFrom(url.searchParams.get('host') ?? undefined, url.searchParams.get('at') ?? undefined);
}

/** {@link reachIn}, from the code's two fields decoded, as a link hands them
 * over. The addresses are apart by spaces, which no address has. */
export function reachFrom(host: string | undefined, at: string | undefined): Reach | null {
  if (!host || !/^[A-Za-z0-9_-]{22}$/.test(host) || !at) return null;
  const addresses = at.split(/\s+/).filter(validRelayUrl);
  return addresses.length ? { hostId: host, addresses: [...new Set(addresses)] } : null;
}

/** `host` knowing `addresses` as well, as if the desktop had listed them: the
 * next snapshot from it replaces the list with what it says then. */
export function addReachable(host: SavedHost, addresses: string[]): SavedHost {
  const known = host.addresses?.length ? host.addresses : [host.relay];
  return { ...host, addresses: [...new Set([...known, ...addresses.filter(validRelayUrl)])] };
}
