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
