use openfatt_core::dec;

use super::*;

fn sample() -> Receipt {
    Receipt {
        lines: vec![
            ReceiptLine { description: "Assira smalto bianco".into(), qty: dec!(1), unit_price: dec!(182.40), discount: dec!(45.24), discount_label: "SCONTO 20+6%".into(), vat_rate: dec!(22), natura: None, department: 1, refund: false },
            ReceiptLine { description: "Caldaia Akassia 30".into(), qty: dec!(2), unit_price: dec!(632.40), discount: dec!(126.48), discount_label: "SCONTO 10%".into(), vat_rate: dec!(10), natura: None, department: 2, refund: false },
        ],
        payments: vec![ReceiptPayment { kind: PayKind::Cash, description: "Contanti".into(), amount: dec!(1275.48) }],
        total: dec!(1275.48),
        change: dec!(0),
        customer_tax_code: "rssmra85t10a562s".into(),
        lottery_code: String::new(),
        courtesy_message: "Grazie e\narrivederci!".into(),
        refund_of: None,
        operator: 1,
    }
}

#[test]
fn ini_format_as_public_specification() {
    let ini = ini::to_ini(&sample(), "COM1", "");
    assert!(ini.starts_with("[General]\r\nVersione=2\r\n"));
    assert!(ini.contains("MsgCortesia=Grazie e#0D#0Aarrivederci!\r\n"), "{}", ini);
    assert!(ini.contains("CodFiscalePartitaIva=RSSMRA85T10A562S\r\n"));
    assert!(ini.contains("TotScontrino=1275.48\r\n"));
    assert!(ini.contains("NumRighe=2\r\nNumPagam=1\r\n"));
    assert!(ini.contains("[Pagam1]\r\nDesc=Contanti\r\nImporto=1275.48\r\nTender=1\r\n"));
    assert!(ini.contains("[Riga1]\r\nDesc=Assira smalto bianco\r\nPercIva=22\r\nReparto=1\r\nReso=False\r\nQta=1\r\nPrezzoUnitario=182.40\r\nDescSconto=SCONTO 20+6%\r\nImportoSconto=45.24\r\n"));
    assert!(ini.contains("[Riga2]\r\nDesc=Caldaia Akassia 30\r\nPercIva=10\r\nReparto=2\r\n"));
}

#[test]
fn epson_xml_totals_match() {
    let xml = epson::receipt_xml(&sample());
    // riga 1: (182,40 - 45,24) / 1 = 137,16; riga 2: (1264,80 - 126,48) / 2 = 569,16
    assert!(xml.contains("unitPrice=\"137,16\" department=\"1\""), "{}", xml);
    assert!(xml.contains("quantity=\"2\" unitPrice=\"569,16\" department=\"2\""), "{}", xml);
    assert!(xml.contains("<printRecTotal operator=\"1\" description=\"Contanti\" payment=\"1275,48\" paymentType=\"0\" index=\"0\""));
    assert!(xml.contains("C.F./P.IVA RSSMRA85T10A562S"));
    assert!(xml.trim_end().ends_with("</s:Envelope>"));
    roxmltree::Document::parse(&xml).unwrap();
}

#[test]
fn epson_refund_has_reference() {
    let mut r = sample();
    r.lines[0].refund = true;
    r.refund_of = Some(("99MEX012345".into(), "0012".into(), "0034".into(), "05-11-2026".into()));
    let xml = epson::receipt_xml(&r);
    assert!(xml.contains("messageType=\"4\" message=\"RESO 0012-0034 del 05-11-2026 99MEX012345\""));
    assert!(xml.contains("<printRecRefund"));
}

#[test]
fn epson_response_parsing() {
    let ok = r#"<?xml version="1.0" encoding="utf-8"?><soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/"><soapenv:Body><response success="true" code="" status="2"><addInfo><elementList>lastCommand,printerStatus,fiscalReceiptNumber,fiscalReceiptAmount,fiscalReceiptDate,fiscalReceiptTime,zRepNumber</elementList><lastCommand>74</lastCommand><printerStatus>20110</printerStatus><fiscalReceiptNumber>34</fiscalReceiptNumber><fiscalReceiptAmount>1.275,48</fiscalReceiptAmount><zRepNumber>12</zRepNumber></addInfo></response></soapenv:Body></soapenv:Envelope>"#;
    let o = epson::parse_response(ok).unwrap();
    assert_eq!(o.receipt_number, "34");
    assert_eq!(o.z_number, "12");
    let ko = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><response success="false" code="EPTR_REC_EMPTY" status="0"/></s:Body></s:Envelope>"#;
    let e = epson::parse_response(ko).unwrap_err();
    assert!(e.0.contains("carta esaurita"));
}

#[test]
fn epson_network_unreachable_is_error() {
    let cfg = RtConfig { driver: Driver::EpsonXml { host: "127.0.0.1".into(), port: 9, timeout_ms: 2000 }, ..Default::default() };
    assert!(emit(&cfg, &sample()).is_err());
}

#[test]
fn simulator_writes_files() {
    let dir = std::env::temp_dir().join(format!("ga-rt-sim-{}", std::process::id()));
    let cfg = RtConfig { driver: Driver::Simulator { folder: dir.display().to_string() }, ..Default::default() };
    let o = emit(&cfg, &sample()).unwrap();
    assert!(o.simulated);
    assert_eq!(o.receipt_number, "0001");
    let o2 = emit(&cfg, &sample()).unwrap();
    assert_eq!(o2.receipt_number, "0002");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn departments() {
    let mut cfg = RtConfig::default();
    cfg.departments.push(DepartmentMap { key: "N4".into(), department: 7 });
    assert_eq!(cfg.department_for(dec!(22), None), 1);
    assert_eq!(cfg.department_for(dec!(10.00), None), 2);
    assert_eq!(cfg.department_for(dec!(0), Some("N4")), 7);
    assert_eq!(cfg.department_for(dec!(0), Some("N2.2")), 1);
}
