import { useEffect, useRef, useState } from "react";
import { Link } from "expo-router";
import * as DocumentPicker from "expo-document-picker";
import { File, Paths } from "expo-file-system";
import { prns_chat_app as app } from "@prns/app-bindings";
import { hex } from "../bytes";
import { useInstallation } from "../runtime/InstallationProvider";
import { Button, Heading, Panel, Screen, Text } from "../ui/primitives";

function message(error: unknown): string { return error instanceof Error ? error.message : String(error); }

/** The picker owns the copy; JavaScript passes only its path, never key bytes. */
function cachePath(uri: string): string {
  const file = new URL(uri);
  const cache = new URL(Paths.cache.uri);
  const path = decodeURIComponent(file.pathname);
  const root = decodeURIComponent(cache.pathname).replace(/\/$/, "");
  if (file.protocol !== "file:" || file.host !== cache.host || file.search !== "" || file.hash !== "" ||
      !path.startsWith(`${root}/`) || path.split("/").some((part) => part === "." || part === "..")) {
    throw new Error("The selected file was not copied into the private cache.");
  }
  return path;
}

export function ImportIdentityScreen() {
  const installation = useInstallation();
  const latest = useRef(installation);
  latest.current = installation;
  const mounted = useRef(false);
  const active = useRef(false);
  const temporary = useRef<File | undefined>(undefined);
  const [selection, setSelection] = useState<app.PhoneIdentityImportSelection>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [cleanupNeeded, setCleanupNeeded] = useState(false);
  const refresh = () => {
    try { setSelection(app.appPendingPhoneIdentityImport()); }
    catch (cause) { setError(message(cause)); }
  };
  useEffect(() => { mounted.current = true; refresh(); return () => { mounted.current = false; }; }, []);

  const removeTemporary = () => {
    temporary.current?.delete();
    temporary.current = undefined;
    if (mounted.current) setCleanupNeeded(false);
  };
  const choose = async () => {
    if (active.current || latest.current.busy !== undefined || latest.current.mode !== "create" || temporary.current !== undefined) return;
    active.current = true; setBusy(true); setError(undefined);
    try {
      const picked = await DocumentPicker.getDocumentAsync({ type: "*/*", multiple: false, copyToCacheDirectory: true });
      if (picked.canceled) return;
      const asset = picked.assets[0];
      if (asset === undefined) throw new Error("No identity file was selected.");
      const path = cachePath(asset.uri);
      temporary.current = new File(asset.uri);
      try {
        if (!mounted.current || latest.current.mode !== "create" || latest.current.busy !== undefined) return;
        const reviewed = app.appReviewPhoneIdentityFile(path);
        // Delete the picker copy before retaining the reviewed native capability.
        removeTemporary();
        const retained = app.appRetainPhoneIdentityImport(reviewed);
        if (mounted.current) setSelection(retained);
      } finally { removeTemporary(); }
    } catch (cause) {
      if (mounted.current) { setError(message(cause)); setCleanupNeeded(temporary.current !== undefined); }
    } finally { active.current = false; if (mounted.current) setBusy(false); }
  };
  const discard = () => {
    if (active.current || selection === undefined) return;
    try { app.appCancelPhoneIdentityImport(selection.id); setError(undefined); refresh(); }
    catch (cause) { setError(message(cause)); }
  };
  const confirm = async () => {
    if (active.current || selection === undefined || latest.current.mode !== "create" || latest.current.busy !== undefined) return;
    active.current = true; setBusy(true); setError(undefined);
    try {
      await installation.startImported(selection.id, installation.bluetooth);
      // The provider records startup failure. Resolution alone does not mean online.
      if (mounted.current) refresh();
    } catch (cause) { if (mounted.current) setError(message(cause)); }
    finally { active.current = false; if (mounted.current) setBusy(false); }
  };
  const unavailable = busy || installation.busy !== undefined || installation.mode !== "create";
  return <Screen><Heading>Use an identity file</Heading>
    <Text>Choose a Reticulum private identity file for this phone’s connections. This does not restore conversations, contacts or messaging identities.</Text>
    <Text>Only import your own identity file. Using it on two active devices at once can cause connection problems.</Text>
    {(error ?? installation.error) !== undefined && <Text accessibilityRole="alert">{error ?? installation.error}</Text>}
    {cleanupNeeded && <Panel><Text>A temporary copy could not be removed. Remove it before choosing another file.</Text>
      <Button title="Remove temporary copy" onPress={() => { try { removeTemporary(); setError(undefined); } catch (cause) { setError(message(cause)); } }} /></Panel>}
    {selection !== undefined && <Panel><Text>Phone identity to import</Text><Text selectable>{hex(selection.identityHash)}</Text>
      <Text>Confirm this identity before continuing. If you close the app first, you will need to choose the file again.</Text>
      <Text>If setup fails after saving this identity, use Reset in Settings before starting over.</Text>
      <Button tone="primary" title="Set up with this identity" disabled={unavailable || cleanupNeeded} onPress={() => { void confirm(); }} />
      <Button title="Discard reviewed identity" disabled={busy || installation.busy !== undefined} onPress={discard} /></Panel>}
    {installation.mode === "create" && <Button title={selection === undefined ? "Choose identity file" : "Choose a different file"}
      disabled={unavailable || cleanupNeeded} onPress={() => { void choose(); }} />}
    {installation.mode === "open" && installation.owner === undefined && <Panel><Text>This phone is already set up. Open its saved data. If setup failed partway, review Reset in Settings before importing again.</Text>
      <Link href="/setup" asChild><Button title="Open saved installation" /></Link></Panel>}
    {installation.owner !== undefined && <Panel><Text>Your phone is ready. Continue to your messaging identities.</Text>
      <Link href="/more/identities" asChild><Button title="Continue to messaging identities" /></Link></Panel>}
  </Screen>;
}
