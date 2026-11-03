import { useEffect, useRef, useState } from "react";
import { Button, SafeAreaView, Text, TextInput, View } from "react-native";
import { balancedLimits, defaultStoragePath, openHost, type OwnedHostSession } from "personal-rns-expo";

export default function App() {
  const session = useRef<OwnedHostSession | undefined>(undefined);
  const [status, setStatus] = useState("Stopped");
  const [peer, setPeer] = useState("192.168.1.10:4242");
  const [busy, setBusy] = useState(false);
  useEffect(() => () => { void session.current?.stop(); }, []);
  async function start() {
    setBusy(true);
    try {
      const root = await defaultStoragePath();
      session.current = await openHost({
        identity: { tag: "LoadOrCreate", data: { path: `${root}/identity` } },
        persistence: { tag: "Directory", data: { path: `${root}/state` } },
        role: "Endpoint", destinations: [], requiredCapabilities: ["TcpClient"], limits: balancedLimits(),
      });
      const snapshot = await session.current.host.snapshot();
      setStatus(`Running: ${snapshot.backend.backend}; ${snapshot.interfaces.length} interfaces`);
    } catch (error) { setStatus(String(error)); }
    finally { setBusy(false); }
  }
  async function connect() {
    setBusy(true);
    try {
      const outcome = await session.current?.host.execute({ tag: "AttachTcpClient", data: { target: peer, bitrate: { tag: "Auto", data: undefined } } });
      setStatus(JSON.stringify(outcome, (_, value: unknown) => typeof value === "bigint" ? value.toString() : value));
    } catch (error) { setStatus(String(error)); }
    finally { setBusy(false); }
  }
  async function stop() {
    setBusy(true);
    try { await session.current?.stop(); session.current = undefined; setStatus("Stopped"); }
    catch (error) { setStatus(String(error)); }
    finally { setBusy(false); }
  }
  return <SafeAreaView><View style={{ padding: 24, gap: 16 }}>
    <Text>General PRNS host</Text><Text>{status}</Text>
    <Button title="Open host" disabled={busy || session.current !== undefined} onPress={() => void start()} />
    <TextInput accessibilityLabel="TCP peer address" value={peer} onChangeText={setPeer} autoCapitalize="none" />
    <Button title="Connect TCP peer" disabled={busy || session.current === undefined} onPress={() => void connect()} />
    <Button title="Stop host" disabled={busy || session.current === undefined} onPress={() => void stop()} />
  </View></SafeAreaView>;
}
