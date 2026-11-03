import { Button, Text, TextInput } from "../ui/primitives";
import { useMemo, useState } from "react";
import { View } from "react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { useStyles } from "../ui";
import { detail, label } from "./presentation";

/** UI checks only lossless integer transport. Rust owns radio/regional rules. */
export function integer(text: string, minimum: number, maximum: number): number | undefined {
  if (!/^-?\d+$/.test(text.trim())) return undefined;
  const value = Number(text);
  return Number.isSafeInteger(value) && value >= minimum && value <= maximum ? value : undefined;
}
const radioLabel = (value: string) => /^Bw\d+kHz$/.test(value) ? value.slice(2, -3) + " kHz" : /^Cr4\d$/.test(value) ? "4/" + value.slice(-1) : label(value);

function Choices<T extends number>({ title, values, selected, choose, disabled }: {
  title: string; values: Record<string, string | number>; selected: T | undefined; choose(value: T): void; disabled: boolean;
}) {
  const styles = useStyles();
  return <View><Text>{title}</Text><View style={styles.row}>{Object.entries(values).filter(([, value]) => typeof value === "number").map(([name, value]) => <Button key={name} title={radioLabel(name)} accessibilityLabel={`${title}: ${radioLabel(name)}`} accessibilityState={{ selected: selected === value }} disabled={disabled} onPress={() => choose(value as T)} />)}</View></View>;
}

export function LoRa({ id, radio, reported, supported, disabled, review }: {
  id: rc.RemoteControlInterfaceId; radio: rc.RemoteControlRadioStatus | undefined; reported: rc.RemoteControlLoRaProfile | undefined;
  supported: rc.RemoteControlRequestSet; disabled: boolean; review(request: rc.RemoteControlRequest): void;
}) {
  const styles = useStyles();
  const [band, setBand] = useState<"SubG" | "Ghz24">("SubG");
  const [region, setRegion] = useState<rc.RemoteControlSubGRegion>();
  const [frequency, setFrequency] = useState("");
  const [power, setPower] = useState("");
  const [preamble, setPreamble] = useState("");
  const [spreadingFactor, setSpreadingFactor] = useState<rc.RemoteControlSpreadingFactor>();
  const [bandwidth, setBandwidth] = useState<rc.RemoteControlLoraBandwidth>();
  const [codingRate, setCodingRate] = useState<rc.RemoteControlCodingRate>();
  const [error, setError] = useState<string>();
  const can = (kind: rc.RemoteControlRequestKind) => supported.kinds.includes(kind);
  const decodedReported = useMemo(() => {
    if (reported === undefined) return undefined;
    try { return rc.remoteControlLoraProfileBand(reported); } catch { return undefined; }
  }, [reported?.value]);
  const supports24 = radio?.tag === rc.RemoteControlRadioStatus_Tags.Status && radio.inner.bands === rc.RemoteControlRadioBands.SubGAndGhz24 || (radio === undefined && decodedReported?.tag === rc.RemoteControlBandProfile_Tags.Ghz24);
  const hz = integer(frequency, 0, 0xffff_ffff);
  const dbm = integer(power, -128, 127);
  const symbols = integer(preamble, 0, 0xffff);
  const complete = hz !== undefined && dbm !== undefined && symbols !== undefined && spreadingFactor !== undefined && bandwidth !== undefined && codingRate !== undefined && (band === "Ghz24" ? supports24 : region !== undefined);
  function load() {
    if (decodedReported === undefined) return;
    try {
      const profile = decodedReported;
      setBand(profile.tag === rc.RemoteControlBandProfile_Tags.SubG ? "SubG" : "Ghz24");
      const value = profile.inner.value;
      setFrequency(value.frequency.value.toString()); setPower(value.txPower.value.toString()); setPreamble(value.preamble.value.toString());
      setSpreadingFactor(value.modulation.inner.spreadingFactor); setBandwidth(value.modulation.inner.bandwidth); setCodingRate(value.modulation.inner.codingRate);
      if (profile.tag === rc.RemoteControlBandProfile_Tags.SubG) setRegion(profile.inner.value.region);
      setError(undefined);
    } catch (cause) { setError(detail(cause)); }
  }
  function prepare(configure: boolean) {
    if (disabled || !complete || hz === undefined || dbm === undefined || symbols === undefined || spreadingFactor === undefined || bandwidth === undefined || codingRate === undefined) return;
    try {
      const modulation = new rc.RemoteControlModulation.Lora({ spreadingFactor, bandwidth, codingRate });
      let profile: rc.RemoteControlBandProfile;
      if (band === "Ghz24") {
        if (!supports24) return;
        const built = rc.remoteControlGhz24Profile({ value: hz }, modulation, { value: dbm }, { value: symbols });
        if (built.tag === rc.RemoteControlGhz24ProfileSettlement_Tags.Failed) throw built.inner.failure;
        profile = new rc.RemoteControlBandProfile.Ghz24({ value: built.inner.value });
      } else {
        if (region === undefined) return;
        const built = rc.remoteControlRadioProfile(region, { value: hz }, modulation, { value: dbm }, { value: symbols });
        if (built.tag === rc.RemoteControlRadioProfileSettlement_Tags.Failed) throw built.inner.failure;
        profile = new rc.RemoteControlBandProfile.SubG({ value: built.inner.value });
      }
      const encoded = rc.remoteControlLoraProfileFromBand(profile);
      if (encoded === undefined) throw new Error("This profile cannot be represented by the node's radio protocol.");
      setError(undefined);
      review(configure ? new rc.RemoteControlRequest.ConfigureRadio({ id, configuration: new rc.RemoteControlRadioConfiguration.Profile({ value: encoded }) }) : new rc.RemoteControlRequest.SetInterfaceLoRaProfile({ id, profile: encoded }));
    } catch (cause) { setError(detail(cause)); }
  }
  if (!can(rc.RemoteControlRequestKind.SetInterfaceLoRaProfile) && !can(rc.RemoteControlRequestKind.ConfigureRadio)) return null;
  return <View style={styles.card}>
    <Text style={styles.title}>LoRa settings</Text>
    <Text>Use compatible settings on the nodes you want to reach. Rust validates the selected band and region; the target separately validates its hardware. Changing these settings can disconnect this radio.</Text>
    {decodedReported !== undefined && <Button title="Copy reported radio profile into editor" disabled={disabled} onPress={load} />}
    <View style={styles.row}><Button title="Sub-GHz" accessibilityState={{ selected: band === "SubG" }} disabled={disabled} onPress={() => setBand("SubG")} />{supports24 && <Button title="2.4 GHz" accessibilityState={{ selected: band === "Ghz24" }} disabled={disabled} onPress={() => setBand("Ghz24")} />}</View>
    {band === "SubG" && <>
      <Choices title="Region" values={rc.RemoteControlRegulatoryRegion} selected={region?.tag === rc.RemoteControlSubGRegion_Tags.Regulated ? region.inner.value : undefined} choose={(value: rc.RemoteControlRegulatoryRegion) => setRegion(new rc.RemoteControlSubGRegion.Regulated({ value }))} disabled={disabled} />
      <Button title="Explicit custom region" accessibilityState={{ selected: region?.tag === rc.RemoteControlSubGRegion_Tags.Custom }} disabled={disabled} onPress={() => setRegion(new rc.RemoteControlSubGRegion.Custom())} />
      {region?.tag === rc.RemoteControlSubGRegion_Tags.Custom && <Text>Custom region uses the core's explicit custom-band policy. Choose settings appropriate for your deployment.</Text>}
    </>}
    <TextInput accessibilityLabel="Radio frequency in Hz" placeholder="Frequency (Hz)" value={frequency} onChangeText={setFrequency} keyboardType="number-pad" style={styles.input} editable={!disabled} />
    <Choices title="Spreading factor" values={rc.RemoteControlSpreadingFactor} selected={spreadingFactor} choose={setSpreadingFactor} disabled={disabled} />
    <Choices title="Bandwidth" values={rc.RemoteControlLoraBandwidth} selected={bandwidth} choose={setBandwidth} disabled={disabled} />
    <Choices title="Coding rate" values={rc.RemoteControlCodingRate} selected={codingRate} choose={setCodingRate} disabled={disabled} />
    <TextInput accessibilityLabel="Transmit power in dBm" placeholder="Transmit power (dBm)" value={power} onChangeText={setPower} keyboardType="numbers-and-punctuation" style={styles.input} editable={!disabled} />
    <TextInput accessibilityLabel="Preamble symbols" placeholder="Preamble symbols" value={preamble} onChangeText={setPreamble} keyboardType="number-pad" style={styles.input} editable={!disabled} />
    {!complete && <Text>Choose the modulation and region, and enter whole numbers in each field.</Text>}
    {can(rc.RemoteControlRequestKind.SetInterfaceLoRaProfile) && <Button title="Validate and review LoRa profile" disabled={disabled || !complete} onPress={() => prepare(false)} />}
    {can(rc.RemoteControlRequestKind.ConfigureRadio) && radio?.tag === rc.RemoteControlRadioStatus_Tags.Status && <Button title="Validate and review saved radio configuration" disabled={disabled || !complete} onPress={() => prepare(true)} />}
    {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
  </View>;
}


export function LoRaFacts({ profile }: { profile: rc.RemoteControlBandProfile | undefined }) {
  if (profile === undefined) return <Text>No supported radio profile.</Text>;
  const value = profile.inner.value;
  const modulation = value.modulation.inner;
  return <View>
    <Text>Band: {profile.tag === rc.RemoteControlBandProfile_Tags.SubG ? "Sub-GHz" : "2.4 GHz"}</Text>
    {profile.tag === rc.RemoteControlBandProfile_Tags.SubG && <Text>Region: {profile.inner.value.region.tag === rc.RemoteControlSubGRegion_Tags.Custom ? "Custom" : label(rc.RemoteControlRegulatoryRegion[profile.inner.value.region.inner.value])}</Text>}
    <Text>Frequency: {value.frequency.value} Hz</Text>
    <Text>Spreading factor: {label(rc.RemoteControlSpreadingFactor[modulation.spreadingFactor])}</Text>
    <Text>Bandwidth: {radioLabel(rc.RemoteControlLoraBandwidth[modulation.bandwidth])}</Text>
    <Text>Coding rate: {radioLabel(rc.RemoteControlCodingRate[modulation.codingRate])}</Text>
    <Text>Transmit power: {value.txPower.value} dBm · preamble: {value.preamble.value} symbols</Text>
  </View>;
}
