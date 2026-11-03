import '@expo/metro-runtime';
import { useState } from 'react';
import { Button, SafeAreaView, Text } from 'react-native';
import { BrowserHost, persistentBrowser } from 'personal-rns-expo/browser';

/** This example uses the existing browser host, worker, storage and capability API. */
export default function App() {
  const [host, setHost] = useState<BrowserHost>();
  const [status, setStatus] = useState('Stopped');
  const start = async () => {
    const moduleUrl = new URL('/prns/prns_wasm.js', window.location.href);
    const outcome = await BrowserHost.create({
      ...persistentBrowser('prns-expo-sdk-example'),
      execution: 'DedicatedWorker',
      wasmModuleUrl: moduleUrl,
    });
    if (outcome.tag === 'Ready') {
      setHost(outcome.data);
      setStatus('Browser host running');
    } else setStatus(`${outcome.tag}: ${JSON.stringify(outcome.data)}`);
  };
  const stop = async () => {
    if (!host) return;
    await host.stop();
    setHost(undefined);
    setStatus('Stopped');
  };
  return <SafeAreaView style={{ padding: 24, gap: 16 }}>
    <Text>PRNS browser host</Text>
    <Text>{status}</Text>
    <Button title="Open host" disabled={host !== undefined} onPress={() => void start()} />
    <Button title="Stop host" disabled={host === undefined} onPress={() => void stop()} />
  </SafeAreaView>;
}
