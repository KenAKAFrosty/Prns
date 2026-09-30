import type * as Bindings from "@prns-internal/expo";
import { useState } from "react";

import { ActionRow, BodyText, CardStack } from "@/ui/primitives";
import { TextField } from "@/ui/text-field";
import { ChoiceField } from "./choice-field";
import { ConfirmAction } from "./confirm-action";
import { loraRegions } from "./interface-format";

export function LoRaEditor({
  profile,
  busy,
  onSave,
}: {
  readonly profile?: Bindings.RemoteLoRaProfile | undefined;
  readonly busy: boolean;
  readonly onSave: (profile: Bindings.RemoteLoRaProfile) => void;
}) {
  const [region, setRegion] = useState<Bindings.RemoteLoRaRegion | undefined>(profile?.region);
  const [fields, setFields] = useState({
    frequencyHz: profile?.frequencyHz.toString() ?? "",
    spreadingFactor: profile?.spreadingFactor.toString() ?? "",
    bandwidthHz: profile?.bandwidthHz.toString() ?? "",
    codingRate: profile?.codingRate.toString() ?? "",
    txPowerDbm: profile?.txPowerDbm.toString() ?? "",
    preambleSymbols: profile?.preambleSymbols.toString() ?? "",
  });
  const inputFields = [
    ["frequencyHz", "Frequency (Hz)", 0, 0xffff_ffff],
    ["spreadingFactor", "Spreading factor", 0, 255],
    ["bandwidthHz", "Bandwidth (Hz)", 0, 0xffff_ffff],
    ["codingRate", "Coding rate (4/x)", 0, 255],
    ["txPowerDbm", "Transmit power (dBm)", -128, 127],
    ["preambleSymbols", "Preamble symbols", 0, 65535],
  ] as const;
  // Only check that values cross the generated numeric bridge losslessly.
  // Regional, radio and profile validation belongs to Rust's domain types.
  const complete = inputFields.every(([name, , minimum, maximum]) => {
    const text = fields[name];
    const value = Number(text);
    return (
      /^-?\d+$/u.test(text) && Number.isSafeInteger(value) && value >= minimum && value <= maximum
    );
  });

  return (
    <CardStack>
      {profile === undefined ? <BodyText>Enter settings for this radio.</BodyText> : null}
      <ChoiceField
        label="Region"
        options={loraRegions}
        value={region}
        onChange={setRegion}
        disabled={busy}
      />
      <ActionRow>
        {inputFields.map(([name, label]) => (
          <TextField
            key={name}
            label={label}
            accessibilityLabel={
              name === "codingRate" ? "Coding rate denominator (5 for 4/5)" : label
            }
            value={fields[name]}
            editable={!busy}
            keyboardType={name === "txPowerDbm" ? "numbers-and-punctuation" : "number-pad"}
            onChangeText={(text) => setFields((previous) => ({ ...previous, [name]: text }))}
          />
        ))}
      </ActionRow>
      <BodyText muted>
        Use the same radio settings on the nodes you want to reach. The node checks whether these
        values are supported.
      </BodyText>
      {!complete ? <BodyText>Enter a whole number in each field.</BodyText> : null}
      {region === undefined ? <BodyText>Choose a region before saving.</BodyText> : null}
      <ConfirmAction
        label="Save LoRa settings"
        warning="This saves a custom radio profile and may replace automatic radio selection. Other nodes may disconnect. Keep another connection available so you can adjust the settings again."
        disabled={busy || !complete || region === undefined}
        onConfirm={() => {
          if (!complete || region === undefined) return;
          onSave({
            region,
            frequencyHz: Number(fields.frequencyHz),
            spreadingFactor: Number(fields.spreadingFactor),
            bandwidthHz: Number(fields.bandwidthHz),
            codingRate: Number(fields.codingRate),
            txPowerDbm: Number(fields.txPowerDbm),
            preambleSymbols: Number(fields.preambleSymbols),
          });
        }}
      />
    </CardStack>
  );
}
