ESP_MEMORY_CONTRACTS = {
    "heltec-v4": {
        "profile": "heltec-v4",
        "regions": {
            "application": (0x00010000, 0x00e7d000),
            "bootloader": (0x00000000, 0x00008000),
            "partition-table": (0x00008000, 0x00009000),
        },
    },
    "heltec-v4-r8": {
        "profile": "heltec-v4-r8",
        "regions": {
            "application": (0x00010000, 0x00e7d000),
            "bootloader": (0x00000000, 0x00008000),
            "partition-table": (0x00008000, 0x00009000),
        },
    },
    "heltec-e290": {
        "profile": "heltec-e290",
        "regions": {
            "application": (0x00010000, 0x00e7d000),
            "bootloader": (0x00000000, 0x00008000),
            "partition-table": (0x00008000, 0x00009000),
        },
    },
    "heltec-wireless-stick-lite-v3": {
        "profile": "heltec-wireless-stick-lite-v3",
        "regions": {
            "application": (0x00010000, 0x0067d000),
            "bootloader": (0x00000000, 0x00008000),
            "partition-table": (0x00008000, 0x00009000),
        },
    },
    "t-beam-supreme": {
        "profile": "t-beam-supreme",
        "regions": {
            "application": (0x00010000, 0x0067d000),
            "bootloader": (0x00000000, 0x00008000),
            "partition-table": (0x00008000, 0x00009000),
        },
    },
    "xiao-esp32-c6": {
        "profile": "xiao-esp32-c6",
        "regions": {
            "application": (0x00010000, 0x003df000),
            "bootloader": (0x00000000, 0x00008000),
            "partition-table": (0x00008000, 0x00009000),
        },
    },
}

UF2_MEMORY_CONTRACTS = {
    ("t-echo", "s140", "6.1.1", "0x00b6", 0x00026000, "0xada52840"): {
        "profile": "t-echo-s140-v6",
        "firmware_owned": (0x00026000, 0x000bf000),
        "transport_envelope": (0x00026000, 0x000c0000),
    },
    ("t-echo", "s140", "7.3.0", "0x0123", 0x00027000, "0xada52840"): {
        "profile": "t-echo-s140-v7",
        "firmware_owned": (0x00027000, 0x000bf000),
        "transport_envelope": (0x00027000, 0x000c0000),
    },
    ("t114", "s140", "6.1.1", "0x00b6", 0x00026000, "0xada52840"): {
        "profile": "t114",
        "firmware_owned": (0x00026000, 0x000e1000),
        "transport_envelope": (0x00026000, 0x000e9000),
    },
    ("mesh-pocket-5000", "s140", "6.1.1", "0x00b6", 0x00026000, "0xada52840"): {
        "profile": "mesh-pocket-5000",
        "firmware_owned": (0x00026000, 0x000e1000),
        "transport_envelope": (0x00026000, 0x000e1000),
    },
    ("mesh-pocket-10000", "s140", "6.1.1", "0x00b6", 0x00026000, "0xada52840"): {
        "profile": "mesh-pocket-10000",
        "firmware_owned": (0x00026000, 0x000e1000),
        "transport_envelope": (0x00026000, 0x000e1000),
    },
    ("t096", "s140", "6.1.1", "0x00b6", 0x00026000, "0xada52840"): {
        "profile": "t096",
        "firmware_owned": (0x00026000, 0x000e1000),
        "transport_envelope": (0x00026000, 0x000e8000),
    },
}

NRF_SERIAL_DFU_MEMORY_CONTRACTS = {
    "t1000-e": {
        "profile": "t1000-e",
        "compatibility": {
            "softdevice_family": "s140",
            "softdevice_version": "7.3.0",
            "fwid": "0x0123",
            "device_type": "0x0052",
            "device_revision": 52840,
            "application_version": "not-enforced",
            "application_base": "0x00027000",
            "application_end_exclusive": "0x000ea000",
            "bank_layout": "single",
        },
        "recovery": {
            "mount_label": "T1000-E",
            "board_id_prefix": "nrf52840-t1000-e-v1",
            "family_id": "0xada52840",
        },
        "firmware_owned": (0x00027000, 0x000e9000),
        "transport_envelope": (0x00027000, 0x000ea000),
    },
}
