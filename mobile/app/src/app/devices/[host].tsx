'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// The phones paired with one desktop: which are connected, which is this one,
// and a way to cut any of them off — this one included, which forgets the
// desktop on both sides.

import { router, useLocalSearchParams } from 'expo-router';
import { useCallback, useEffect, useState } from 'react';
import { ScrollView, StyleSheet, TextInput, View } from 'react-native';
import type { Device } from '@ket/remote';

import { Bar, Button, Card, Dot, Heading, Prose, Row, Screen, field } from '../../components/ui';
import { confirm } from '../../lib/confirm';
import { dropConnection, type HostConnection } from '../../lib/connection';
import { useHost } from '../../lib/hosts';
import { forgetHost } from '../../lib/store';
import { size, t } from '../../lib/theme';

export default function Devices() {
  const { host: id } = useLocalSearchParams<{ host: string }>();
  const conn = useHost(id);
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [error, setError] = useState('');
  const [address, setAddress] = useState('');
  const [addressError, setAddressError] = useState('');

  const load = useCallback(async (c: HostConnection) => {
    try {
      setDevices(await c.devices());
      setError('');
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    if (!conn) return;
    let live = true;
    conn
      .devices()
      .then((list) => {
        if (!live) return;
        setDevices(list);
        setError('');
      })
      .catch((e: unknown) => {
        if (live) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      live = false;
    };
  }, [conn]);

  const revoke = async (device: Device) => {
    if (!conn) return;
    const yes = await confirm(`Revoke ${device.name || 'this phone'}?`, 'It will be disconnected and have to pair again.', 'Revoke');
    if (!yes) return;
    await conn.revoke(device.id).then(() => load(conn)).catch((e: Error) => setError(e.message));
  };

  const forget = async () => {
    const yes = await confirm(`Forget ${conn?.host.name ?? 'this desktop'}?`, 'This phone will have to pair again to use it.', 'Forget');
    if (!yes) return;
    const self = devices?.find((d) => d.thisDevice);
    // Tell the desktop first, while there is still a session to say it in; the
    // phone forgets the desktop whether or not that works.
    if (conn && self) await conn.revoke(self.id).catch(() => {});
    dropConnection(id);
    await forgetHost(id);
    router.replace('/settings');
  };

  const addAddress = async () => {
    if (!conn) return;
    try {
      await conn.addAddress(address);
      setAddress('');
      setAddressError('');
    } catch (cause) {
      setAddressError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const removeAddress = async (url: string) => {
    if (!conn) return;
    await conn.removeAddress(url).catch((cause: Error) => setAddressError(cause.message));
  };

  return (
    <Screen>
      <Bar back title="Devices" detail={conn?.host.name} />
      <ScrollView contentContainerStyle={styles.scroll}>
        {error ? (
          <Card style={styles.card}>
            <Prose color={t.failed} style={styles.note}>
              {error}
            </Prose>
          </Card>
        ) : null}
        <Card style={styles.card}>
        <Heading count={devices?.length}>Paired phones</Heading>
          {(devices ?? []).map((device) => (
            <Row
              key={device.id}
              mark={<Dot color={device.connected ? t.running : t.faint} />}
              title={`${device.name || 'Unnamed phone'}${device.thisDevice ? ' (this one)' : ''}`}
              detail={`Paired ${new Date(Number(device.pairedAt) * 1000).toLocaleDateString()}${device.connected ? ' · connected' : ''}`}
              small
              onPress={device.thisDevice ? undefined : () => void revoke(device)}
            />
          ))}
        {devices === null && !error ? <Prose style={styles.note}>Asking the desktop…</Prose> : null}
        </Card>
        <Card style={styles.card}>
          <Heading>Addresses</Heading>
          <Prose style={styles.note}>The Mac shares its LAN and Tailscale addresses. Add another address for a work VPN, port forwarding, or your own relay.</Prose>
          {(conn?.host.addresses ?? [conn?.host.relay].filter((url): url is string => !!url)).map((url) => (
            <Row key={`host-${url}`} small title={url} detail="From your Mac" />
          ))}
          {(conn?.host.manualAddresses ?? []).map((url) => (
            <View key={`manual-${url}`} style={styles.addressRow}>
              <View style={styles.addressText}><Row small title={url} detail="Added on this phone" /></View>
              <Button title="Remove" onPress={() => void removeAddress(url)} />
            </View>
          ))}
          <TextInput
            style={field}
            value={address}
            onChangeText={setAddress}
            placeholder="ws:// or wss:// address"
            placeholderTextColor={t.dim}
            autoCapitalize="none"
            autoCorrect={false}
            keyboardType="url"
            accessibilityLabel="New desktop address"
          />
          {addressError ? <Prose color={t.failed} style={styles.note}>{addressError}</Prose> : null}
          <View style={styles.footer}><Button title="Add address" onPress={() => void addAddress()} /></View>
        </Card>
        <View style={styles.footer}>
          <Button kind="danger" title="Forget this desktop" onPress={() => void forget()} />
        </View>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  scroll: { padding: size.gutter, paddingTop: 0, gap: size.gutter, paddingBottom: 32 },
  card: { padding: 8, gap: 2 },
  note: { padding: 8 },
  footer: { alignItems: 'flex-start' },
  addressRow: { flexDirection: 'row', alignItems: 'center' },
  addressText: { flex: 1 },
});
