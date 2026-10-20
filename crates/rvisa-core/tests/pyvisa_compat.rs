//! Test cases ported from pyvisa's test suite, to check that `rvisa-core`
//! agrees with pyvisa where their scopes overlap.
//!
//! Source: <https://github.com/pyvisa/pyvisa> at commit `afd2574`
//! (pyvisa 1.16.2), `pyvisa/testsuite/test_rname.py` and
//! `pyvisa/testsuite/test_util.py`. pyvisa is MIT-licensed, Copyright (c)
//! 2005-2024 PyVISA Authors and contributors.
//!
//! The ported cases keep pyvisa's inputs. Where `rvisa` deliberately
//! behaves differently, the test asserts `rvisa`'s behaviour and says why
//! (see DESIGN.md X4). Cases for features outside this library's scope
//! (ROADMAP §3) are listed at the end of each module rather than ported.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod rname {
    //! `test_rname.py`: resource-name parsing.

    use rvisa_core::ResourceName;

    /// Parses `input`, checks the result, and checks the round trip.
    fn parses(input: &str, expected: &ResourceName, canonical: &str) {
        let parsed: ResourceName = input
            .parse()
            .unwrap_or_else(|e| panic!("{input:?} should parse: {e}"));
        assert_eq!(&parsed, expected, "{input:?}");
        assert_eq!(parsed.to_string(), canonical, "{input:?}");
        assert_eq!(
            canonical.parse::<ResourceName>().unwrap(),
            parsed,
            "{canonical:?} should round-trip"
        );
    }

    fn rejects(input: &str) {
        assert!(
            input.parse::<ResourceName>().is_err(),
            "{input:?} should be rejected"
        );
    }

    fn tcpip(board: u16, host: &str, device: Option<&str>) -> ResourceName {
        ResourceName::TcpipInstr {
            board,
            host: host.into(),
            device: device.map(Into::into),
        }
    }

    fn socket(board: u16, host: &str, port: u16) -> ResourceName {
        ResourceName::TcpipSocket {
            board,
            host: host.into(),
            port,
        }
    }

    fn usb(board: u16, interface: Option<u16>) -> ResourceName {
        ResourceName::UsbInstr {
            board,
            vid: 0x1234,
            pid: 125,
            serial: "A22-5".into(),
            interface,
        }
    }

    fn gpib(board: u16, primary: u8, secondary: Option<u8>) -> ResourceName {
        ResourceName::GpibInstr {
            board,
            primary,
            secondary,
        }
    }

    // TestParsers.test_asrl
    #[test]
    fn asrl() {
        let asrl1 = ResourceName::AsrlInstr { board: 1 };
        parses("ASRL1::INSTR", &asrl1, "ASRL1::INSTR");
        parses("ASRL1", &asrl1, "ASRL1::INSTR");
    }

    // TestParsers.test_gpib_instr
    #[test]
    fn gpib_instr() {
        parses(
            "GPIB::1::1::INSTR",
            &gpib(0, 1, Some(1)),
            "GPIB0::1::1::INSTR",
        );
        parses("GPIB::1::INSTR", &gpib(0, 1, None), "GPIB0::1::INSTR");
        parses("GPIB1::1::INSTR", &gpib(1, 1, None), "GPIB1::1::INSTR");
        parses("GPIB1::1", &gpib(1, 1, None), "GPIB1::1::INSTR");
    }

    // TestParsers.test_tcpip_intr
    //
    // pyvisa fills in the default LAN device name, giving
    // `TCPIP0::192.168.134.102::inst0::INSTR`. rvisa keeps it absent
    // (`device: None`), and the backend applies the default.
    #[test]
    fn tcpip_instr() {
        parses(
            "TCPIP::192.168.134.102",
            &tcpip(0, "192.168.134.102", None),
            "TCPIP0::192.168.134.102::INSTR",
        );
        parses(
            "TCPIP::dev.company.com::INSTR",
            &tcpip(0, "dev.company.com", None),
            "TCPIP0::dev.company.com::INSTR",
        );
        parses(
            "TCPIP3::dev.company.com::inst3::INSTR",
            &tcpip(3, "dev.company.com", Some("inst3")),
            "TCPIP3::dev.company.com::inst3::INSTR",
        );
        parses(
            "TCPIP3::1.2.3.4::inst3::INSTR",
            &tcpip(3, "1.2.3.4", Some("inst3")),
            "TCPIP3::1.2.3.4::inst3::INSTR",
        );
    }

    // TestParsers.test_tcpip_instr_names
    #[test]
    fn tcpip_instr_names() {
        for name in [
            "TCPIP::127.0.0.1::inst0::INSTR",
            "TCPIP::127.0.0.1::hislip0,4880::INSTR",
            "TCPIP::127.0.0.1,111::inst0::INSTR",
        ] {
            let parsed: ResourceName = name.parse().unwrap();
            assert!(matches!(parsed, ResourceName::TcpipInstr { .. }), "{name}");
            assert_eq!(parsed.to_string(), name.replacen("TCPIP::", "TCPIP0::", 1));
        }
    }

    // TestParsers.test_tcpip_socket
    #[test]
    fn tcpip_socket() {
        parses(
            "TCPIP::1.2.3.4::999::SOCKET",
            &socket(0, "1.2.3.4", 999),
            "TCPIP0::1.2.3.4::999::SOCKET",
        );
        parses(
            "TCPIP2::1.2.3.4::999::SOCKET",
            &socket(2, "1.2.3.4", 999),
            "TCPIP2::1.2.3.4::999::SOCKET",
        );
    }

    // TestParsers.test_tcpip_socket_valid_port
    //
    // pyvisa keeps the port as text, so `05025` stays `05025`. rvisa stores
    // a `u16` and writes `5025`.
    #[test]
    fn tcpip_socket_valid_port() {
        for (text, port) in [("1", 1), ("65535", 65535), ("05025", 5025)] {
            parses(
                &format!("TCPIP::127.0.0.1::{text}::SOCKET"),
                &socket(0, "127.0.0.1", port),
                &format!("TCPIP0::127.0.0.1::{port}::SOCKET"),
            );
        }
    }

    // pyvisa accepts these ports because it validates them with Python's
    // `int()`. rvisa rejects them: port 0 cannot be connected to, and the
    // grammar has no sign or whitespace inside a component (DESIGN.md C2).
    #[test]
    fn tcpip_socket_ports_pyvisa_accepts() {
        for port in ["0", "+5025", " 5025 "] {
            rejects(&format!("TCPIP::127.0.0.1::{port}::SOCKET"));
        }
    }

    // TestParsers.test_tcpip_socket_invalid_port
    #[test]
    fn tcpip_socket_invalid_port() {
        for port in ["notaport", "5000.5", "-1", "65536"] {
            rejects(&format!("TCPIP::127.0.0.1::{port}::SOCKET"));
        }
    }

    // TestParsers.test_tcpip_socket_required_fields
    #[test]
    fn tcpip_socket_required_fields() {
        for name in [
            "TCPIP::127.0.0.1::SOCKET",
            "TCPIP::127.0.0.1::::SOCKET",
            "TCPIP::::5025::SOCKET",
        ] {
            rejects(name);
        }
    }

    // TestParsers.test_usb_instr
    //
    // pyvisa keeps the IDs as text and fills in interface 0, giving
    // `USB0::0x1234::125::A22-5::0::INSTR`. rvisa stores the IDs as numbers
    // written `0x%04X`, and keeps an absent interface absent.
    #[test]
    fn usb_instr() {
        parses(
            "USB::0x1234::125::A22-5::INSTR",
            &usb(0, None),
            "USB0::0x1234::0x007D::A22-5::INSTR",
        );
        parses(
            "USB2::0x1234::125::A22-5::INSTR",
            &usb(2, None),
            "USB2::0x1234::0x007D::A22-5::INSTR",
        );
        parses(
            "USB::0x1234::125::A22-5",
            &usb(0, None),
            "USB0::0x1234::0x007D::A22-5::INSTR",
        );
        parses(
            "USB::0x1234::125::A22-5::3::INSTR",
            &usb(0, Some(3)),
            "USB0::0x1234::0x007D::A22-5::3::INSTR",
        );
        parses(
            "USB2::0x1234::125::A22-5::3::INSTR",
            &usb(2, Some(3)),
            "USB2::0x1234::0x007D::A22-5::3::INSTR",
        );
        parses(
            "USB1::0x1234::125::A22-5::3",
            &usb(1, Some(3)),
            "USB1::0x1234::0x007D::A22-5::3::INSTR",
        );
    }

    // TestResourceName.test_creation_from_string
    #[test]
    fn creation_from_string_errors() {
        for name in [
            "RJ45::1",               // unknown interface type
            "PXI::1",                // no default resource class
            "GPIB",                  // not enough parts
            "GPIB1::1::1::1::INSTR", // too many parts
            "GPIB::::INSTR",         // missing mandatory part
        ] {
            rejects(name);
        }
    }

    // TestResourceName.test_creation_from_string also rejects a bare "ASRL"
    // because pyvisa requires the board number. VPP-4.3 makes it optional,
    // defaulting to 0, so rvisa accepts it (DESIGN.md C2).
    #[test]
    fn bare_asrl_is_board_zero() {
        parses(
            "ASRL",
            &ResourceName::AsrlInstr { board: 0 },
            "ASRL0::INSTR",
        );
    }

    // TestParsers.test_gpib_intf, test_prlgx_intf, test_ni_enet100_intf,
    // test_vicp_intr and test_usb_raw parse resource classes and interface
    // types this library does not support (ROADMAP §3). They must be
    // rejected cleanly.
    #[test]
    fn unsupported_forms_are_rejected() {
        for name in [
            "GPIB::INTFC",
            "GPIB3::INTFC",
            "PRLGX-TCPIP::1.2.3.4::INTFC",
            "PRLGX-TCPIP::169.254.1.80::23::INTFC",
            "PRLGX-ASRL2::/dev/cu.usbserial-5678::INTFC",
            "NI-ENET100-TCPIP::1.2.3.4::INTFC",
            "VICP::192.168.134.102",
            "VICP::dev.company.com::INSTR",
            "USB::0x1234::125::A22-5::RAW",
            "USB2::0x1234::125::A22-5::3::RAW",
        ] {
            rejects(name);
        }
    }

    // Not ported: TestInvalidResourceName and TestRegisteringSubclass test
    // pyvisa's error message wording and its parser registry, and
    // test_creation_from_kwargs and test_accessing_interface_type test
    // pyvisa's Python API. rvisa has no equivalents.
}

mod filters {
    //! `test_rname.py` `TestFilters`: VISA search expressions.

    use rvisa_core::Pattern;

    const RUN_LIST: [&str; 16] = [
        "GPIB0::8::INSTR",
        "TCPIP0::localhost:1111::inst0::INSTR",
        "ASRL1::INSTR",
        "USB1::0x1111::0x2222::0x4445::0::RAW",
        "USB0::0x1112::0x2223::0x1234::0::INSTR",
        "TCPIP0::192.168.0.1::inst1::INSTR",
        "TCPIP0::localhost::10001::SOCKET",
        "GPIB9::7::1::INSTR",
        "ASRL11::INSTR",
        "ASRL2::INSTR",
        "GPIB::INTFC",
        "PXI::1::BACKPLANE",
        "PXI::MEMACC",
        "VXI::1::BACKPLANE",
        "VXI::1::INSTR",
        "VXI::SERVANT",
    ];

    /// pyvisa's `_test_filter`: the indices in `RUN_LIST` that match.
    fn check(expr: &str, expected: &[usize]) {
        let pattern = Pattern::new(expr).unwrap();
        let matched: Vec<usize> = (0..RUN_LIST.len())
            .filter(|&i| pattern.matches_str(RUN_LIST[i]))
            .collect();
        assert_eq!(matched, expected, "{expr:?}");
    }

    // TestFilters.test_filter and test_filter2_no_optional_clause
    #[test]
    fn filter() {
        check("?*::INSTR", &[0, 1, 2, 4, 5, 7, 8, 9, 14]);
        check("GPIB?+INSTR", &[0, 7]);
        check("GPIB[0-8]*::?*INSTR", &[0]);
        check("GPIB[^0]::?*INSTR", &[7]);
        check("ASRL1+::INSTR", &[2, 8]);
        check("(GPIB|VXI)?*INSTR", &[0, 7, 14]);
    }

    // TestFilters.test_bad_filter: pyvisa raises VI_ERROR_INV_EXPR.
    #[test]
    fn bad_filter() {
        assert!(Pattern::new("?*(").is_err());
    }

    // Not ported: the attribute clause `{VI_ATTR_... == ...}` in
    // test_filter (`"?*{}"`), test_filter2_optional_clause_no_connection
    // and test_bad_filter2. `Pattern` matches names only; the vendor
    // backend passes the whole expression to `viFindRsrc` (DESIGN.md X4).
}

mod ieee_block {
    //! `test_util.py` `TestParser`: IEEE 488.2 arbitrary block data.
    //!
    //! rvisa returns a block's bytes and leaves decoding them to the caller,
    //! so only the framing cases are ported.

    use rvisa_core::ieee488::{encode_header, parse_block};

    /// The 56-byte payload of pyvisa's `test_parse_binary`: fourteen
    /// little-endian `f32`s.
    const PAYLOAD: &[u8] = b"@\xe2\x8b<@\xe2\x8b<@\xe2\x8b<@\xe2\x8b<@\xde\x8b<@\xde\x8b<@\
\xde\x8b<@\xde\x8b<@\xe0\x8b<@\xe0\x8b<@\xdc\x8b<@\xde\x8b<\
@\xe2\x8b<@\xe0\x8b<";

    const EXPECTED: [f64; 14] = [
        0.017_075_66,
        0.017_075_66,
        0.017_075_66,
        0.017_075_66,
        0.017_073_75,
        0.017_073_75,
        0.017_073_75,
        0.017_073_75,
        0.017_074_70,
        0.017_074_70,
        0.017_072_80,
        0.017_073_75,
        0.017_075_66,
        0.017_074_70,
    ];

    fn block(header: &[u8], data: &[u8]) -> Vec<u8> {
        [header, data].concat()
    }

    fn decode_f32_le(data: &[u8]) -> Vec<f32> {
        data.chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    // TestParser.test_parse_binary (IEEE header forms)
    #[test]
    fn parse_binary() {
        assert_eq!(PAYLOAD.len(), 56);

        // Indefinite length.
        let values = decode_f32_le(parse_block(&block(b"#0", PAYLOAD)).unwrap());
        assert_eq!(values.len(), EXPECTED.len());
        for (got, want) in values.iter().zip(EXPECTED) {
            assert!((f64::from(*got) - want).abs() < 1e-8, "{got} != {want}");
        }

        // Definite length.
        assert_eq!(parse_block(&block(b"#256", PAYLOAD)).unwrap(), PAYLOAD);

        // Zero length.
        assert!(parse_block(&block(b"#10", PAYLOAD)).unwrap().is_empty());
    }

    // TestParser.test_no_start_of_block_indicator_binary_block_header
    #[test]
    fn no_start_of_block_indicator() {
        let good = block(b"#220", &[0; 20]);
        assert!(parse_block(&good[1..]).is_err());
    }

    // TestParser.test_late_binary_block_header and
    // test_late_valid_binary_block_header_is_decoded: pyvisa warns when the
    // header starts more than 25 bytes in, but still decodes it. rvisa skips
    // anything before the `#`.
    #[test]
    fn late_header_is_decoded() {
        let data: Vec<u8> = (0..20).collect();
        let message = block(&[b'x'; 26], &block(b"#220", &data));
        assert_eq!(parse_block(&message).unwrap(), data);
    }

    // TestParser.test_binary_block_shorter_than_advertized
    #[test]
    fn shorter_than_advertised() {
        let data = [0u8; 198];
        assert!(parse_block(&block(b"#3999", &data)).is_err());
    }

    // TestParser.test_guessing_block_length: a length field of zeros
    // declares an empty block.
    #[test]
    fn zero_length_field() {
        let data = [1u8; 198];
        assert!(parse_block(&block(b"#3000", &data)).unwrap().is_empty());
    }

    // TestParser.test_bytes_binary_block and
    // test_block_header_length_matches_payload: the declared length is the
    // number of bytes that follow, and the block round-trips.
    #[test]
    fn header_matches_payload() {
        let data = b"dbslbw cj saj \x00\x76";
        let header = encode_header(data.len()).unwrap();
        assert_eq!(header, "#216");
        let message = block(header.as_bytes(), data);
        assert_eq!(parse_block(&message).unwrap(), data);
    }

    // TestParser.test_block_length_too_large_for_header: a length needing
    // more than nine digits cannot be written in an IEEE header.
    #[test]
    fn length_too_large_for_header() {
        assert_eq!(encode_header(999_999_999).unwrap(), "#9999999999");
        assert!(encode_header(1_000_000_000).is_err());
    }

    // Not ported: HP (`#A`) and R&S (`#(n)`) headers, element decoding
    // (datatypes, endianness, partial elements, containers) and ASCII
    // blocks. rvisa reads IEEE 488.2 blocks only and returns raw bytes.
}
