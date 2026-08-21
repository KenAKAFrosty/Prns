using System.Collections.Immutable;
using System.Runtime.InteropServices;

namespace PersonalRns;

internal static class InterfaceConfigMarshaller
{
    internal static Native.InterfaceConfig Marshal(
        InterfaceConfig value,
        NativeArena arena
    ) => value.Match(
        autoLan => AutoLan(autoLan, arena),
        tcpClient => WithBitrate(InterfaceKind.TcpClient, tcpClient.Bitrate, target: arena.String(tcpClient.Target)),
        tcpServer => WithBitrate(InterfaceKind.TcpServer, tcpServer.Bitrate, bind: arena.String(tcpServer.Bind)),
        udp => WithBitrate(
            InterfaceKind.Udp,
            udp.Bitrate,
            local: arena.String(udp.Local),
            peer: arena.String(udp.Peer)
        ),
        serial => new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.Serial,
            Port = arena.String(serial.Port),
            Line = SerialLine(serial.Line),
        },
        kiss => Kiss(kiss, arena),
        ax25Kiss => Ax25Kiss(ax25Kiss, arena),
        rNode => RNode(rNode, arena),
        multiRNode => MultiRNode(multiRNode, arena),
        pipe => Pipe(pipe, arena),
        backboneClient => WithBitrate(
            InterfaceKind.BackboneClient,
            backboneClient.Bitrate,
            target: arena.String(backboneClient.Target)
        ),
        backboneServer => WithBitrate(
            InterfaceKind.BackboneServer,
            backboneServer.Bitrate,
            bind: arena.String(backboneServer.Bind)
        ),
        i2p => I2p(i2p, arena),
        weave => new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.Weave,
            Port = arena.String(weave.Port),
        },
        _ => new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.AutomaticUsb,
        },
        _ => new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.AutomaticBluetoothLe,
        },
        client => new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.WebSocketClient,
            Target = arena.String(client.Target),
            WebSocketFramingSelection = client.Framing,
        },
        server => new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.WebSocketServer,
            Bind = arena.String(server.Bind),
            WebSocketFramingSelection = server.Framing,
        },
        rendezvous => new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.BrowserRendezvous,
            Url = arena.String(rendezvous.Url),
        }
    );

    private static nuint Size => (nuint)System.Runtime.InteropServices.Marshal.SizeOf<Native.InterfaceConfig>();

    private static Native.InterfaceConfig AutoLan(
        InterfaceConfig.AutoLan value,
        NativeArena arena
    )
    {
        var devices = Strings(value.Devices, arena);
        var ignoredDevices = Strings(value.IgnoredDevices, arena);
        return new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.AutoLan,
            HasGroupId = Flag(value.GroupId is not null),
            GroupId = value.GroupId is null ? default : arena.String(value.GroupId),
            HasDiscoveryScope = Flag(value.DiscoveryScope.HasValue),
            DiscoveryScope = value.DiscoveryScope.GetValueOrDefault(),
            HasDiscoveryPort = Flag(value.DiscoveryPort.HasValue),
            DiscoveryPort = value.DiscoveryPort.GetValueOrDefault(),
            HasDataPort = Flag(value.DataPort.HasValue),
            DataPort = value.DataPort.GetValueOrDefault(),
            Devices = arena.Array<Native.StringView>(devices),
            DeviceCount = (nuint)devices.Length,
            IgnoredDevices = arena.Array<Native.StringView>(ignoredDevices),
            IgnoredDeviceCount = (nuint)ignoredDevices.Length,
            HasMulticastAddressType = Flag(value.MulticastAddressType.HasValue),
            MulticastAddressType = value.MulticastAddressType.GetValueOrDefault(),
        };
    }

    private static Native.InterfaceConfig Kiss(
        InterfaceConfig.Kiss value,
        NativeArena arena
    )
    {
        var result = new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.Kiss,
            Port = arena.String(value.Port),
            Line = SerialLine(value.Line),
            FlowControl = Flag(value.FlowControl),
            PreambleMillis = value.PreambleMillis,
            TransmitTailMillis = value.TransmitTailMillis,
            Persistence = value.Persistence,
            SlotTimeMillis = value.SlotTimeMillis,
        };
        ApplyStation(value.StationCallsign, value.StationIntervalSeconds, arena, ref result);
        return result;
    }

    private static Native.InterfaceConfig Ax25Kiss(
        InterfaceConfig.Ax25Kiss value,
        NativeArena arena
    ) => new()
    {
        StructSize = Size,
        Kind = InterfaceKind.Ax25Kiss,
        Port = arena.String(value.Port),
        Line = SerialLine(value.Line),
        FlowControl = Flag(value.FlowControl),
        PreambleMillis = value.PreambleMillis,
        TransmitTailMillis = value.TransmitTailMillis,
        Persistence = value.Persistence,
        SlotTimeMillis = value.SlotTimeMillis,
        Callsign = arena.String(value.Callsign),
        Ssid = value.Ssid,
    };

    private static Native.InterfaceConfig RNode(
        InterfaceConfig.RNode value,
        NativeArena arena
    )
    {
        var result = new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.RNode,
            Port = arena.String(value.Port),
            Radio = Radio(value.Radio),
            FlowControl = Flag(value.FlowControl),
            HasAirtimeLimitShortCentiPercent = Flag(value.AirtimeLimitShortCentiPercent.HasValue),
            AirtimeLimitShortCentiPercent = value.AirtimeLimitShortCentiPercent.GetValueOrDefault(),
            HasAirtimeLimitLongCentiPercent = Flag(value.AirtimeLimitLongCentiPercent.HasValue),
            AirtimeLimitLongCentiPercent = value.AirtimeLimitLongCentiPercent.GetValueOrDefault(),
        };
        ApplyStation(value.StationCallsign, value.StationIntervalSeconds, arena, ref result);
        return result;
    }

    private static Native.InterfaceConfig MultiRNode(
        InterfaceConfig.MultiRNode value,
        NativeArena arena
    )
    {
        var members = new Native.MultiRNodeMemberConfig[value.Members.Length];
        for (var index = 0; index < members.Length; index++)
        {
            var member = value.Members[index];
            members[index] = new Native.MultiRNodeMemberConfig
            {
                StructSize = (nuint)System.Runtime.InteropServices.Marshal.SizeOf<Native.MultiRNodeMemberConfig>(),
                Name = arena.String(member.Name),
                VirtualPort = member.VirtualPort,
                Radio = Radio(member.Radio),
                FlowControl = Flag(member.FlowControl),
                Outgoing = Flag(member.Outgoing),
            };
        }
        var result = new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.MultiRNode,
            Port = arena.String(value.Port),
            Members = arena.Array<Native.MultiRNodeMemberConfig>(members),
            MemberCount = (nuint)members.Length,
        };
        ApplyStation(value.StationCallsign, value.StationIntervalSeconds, arena, ref result);
        return result;
    }

    private static Native.InterfaceConfig Pipe(
        InterfaceConfig.Pipe value,
        NativeArena arena
    )
    {
        var command = Strings(value.Command, arena);
        return new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.Pipe,
            Command = arena.Array<Native.StringView>(command),
            CommandCount = (nuint)command.Length,
            RespawnDelayMillis = value.RespawnDelayMillis,
        };
    }

    private static Native.InterfaceConfig I2p(
        InterfaceConfig.I2p value,
        NativeArena arena
    )
    {
        var peers = Strings(value.Peers, arena);
        return new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = InterfaceKind.I2p,
            Peers = arena.Array<Native.StringView>(peers),
            PeerCount = (nuint)peers.Length,
            Connectable = Flag(value.Connectable),
        };
    }

    private static Native.InterfaceConfig WithBitrate(
        InterfaceKind kind,
        Bitrate value,
        Native.StringView target = default,
        Native.StringView bind = default,
        Native.StringView local = default,
        Native.StringView peer = default
    )
    {
        var bitrate = value.Match(
            _ => (BitrateKind.Auto, 0UL),
            bits => (BitrateKind.BitsPerSecond, bits.Value)
        );
        return new Native.InterfaceConfig
        {
            StructSize = Size,
            Kind = kind,
            Target = target,
            Bind = bind,
            Local = local,
            Peer = peer,
            BitrateKind = bitrate.Item1,
            BitrateBps = bitrate.Item2,
        };
    }

    private static Native.SerialLineConfig SerialLine(SerialLineConfig value) => new()
    {
        StructSize = (nuint)System.Runtime.InteropServices.Marshal.SizeOf<Native.SerialLineConfig>(),
        Baud = value.Baud,
        DataBits = value.DataBits,
        Parity = value.Parity,
        StopBits = value.StopBits,
    };

    private static Native.RNodeRadioConfig Radio(RNodeRadioConfig value) => new()
    {
        StructSize = (nuint)System.Runtime.InteropServices.Marshal.SizeOf<Native.RNodeRadioConfig>(),
        FrequencyHz = value.FrequencyHz,
        BandwidthHz = value.BandwidthHz,
        TxPowerDbm = value.TxPowerDbm,
        SpreadingFactor = value.SpreadingFactor,
        CodingRate = value.CodingRate,
    };

    private static Native.StringView[] Strings(
        ImmutableArray<string> values,
        NativeArena arena
    )
    {
        var result = new Native.StringView[values.Length];
        for (var index = 0; index < result.Length; index++)
        {
            result[index] = arena.String(values[index]);
        }
        return result;
    }

    private static void ApplyStation(
        string? callsign,
        ulong? interval,
        NativeArena arena,
        ref Native.InterfaceConfig result
    )
    {
        if (callsign is not null)
        {
            result.HasStationCallsign = 1;
            result.StationCallsign = arena.String(callsign);
        }
        if (interval.HasValue)
        {
            result.HasStationIntervalSeconds = 1;
            result.StationIntervalSeconds = interval.Value;
        }
    }

    private static byte Flag(bool value) => value ? (byte)1 : (byte)0;
}
