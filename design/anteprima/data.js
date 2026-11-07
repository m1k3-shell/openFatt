// Dati di esempio dell'anteprima: Bottega Demo S.r.l., anno 2026, oggi venerdì 7 novembre 2026.
// Ogni totale mostrato a schermo è calcolato da queste righe (nessun numero scritto a mano altrove).
window.GA = (() => {
  const TODAY = '2026-11-07';
  const r2 = (v) => Math.round((v + Number.EPSILON) * 100) / 100;

  // stato SdI: gen = XML da generare, wait = inviata in attesa di esito, ok = consegnata, ko = scartata
  const invoices = [
    { n: 1, date: '2026-06-15', party: 'Hotel Lago Blu S.r.l.', imp: 2450.00, pay: 'Bonifico 30 gg', dues: [['2026-07-15', null]], paid: true, sdi: 'ok', sdiDate: '2026-06-16' },
    { n: 2, date: '2026-07-03', party: 'Studio Tecnico Ferri', imp: 612.00, pay: 'Bonifico vista fattura', dues: [['2026-07-03', null]], paid: true, sdi: 'ok', sdiDate: '2026-07-04' },
    { n: 3, date: '2026-07-22', party: 'Rossi Arredamenti S.p.A.', imp: 1380.00, pay: 'RiBa 30 gg f.m.', dues: [['2026-08-31', null]], paid: true, sdi: 'ok', sdiDate: '2026-07-23' },
    { n: 4, date: '2026-09-09', party: 'Comune di Valverde', imp: 890.00, pay: 'Bonifico 30 gg', dues: [['2026-10-09', null]], paid: true, sdi: 'ok', sdiDate: '2026-09-11', pa: true },
    { n: 5, date: '2026-09-30', party: 'Bar Centrale di Gatti Luca', imp: 318.50, pay: 'Bonifico 30 gg', dues: [['2026-10-30', null]], paid: false, sdi: 'ok', sdiDate: '2026-10-01' },
    { n: 6, date: '2026-10-20', party: 'Rossi Arredamenti S.p.A.', imp: 1777.40, pay: 'RiBa 30-60 gg d.f. f.m.', dues: [['2026-11-30', 1084.22], ['2026-12-31', 1084.21]], paid: false, sdi: 'ok', sdiDate: '2026-10-21' },
    { n: 7, date: '2026-11-02', party: 'Mario Bianchi', imp: 108.03, pay: 'Bonifico 30 gg', dues: [['2026-12-02', null]], paid: false, sdi: 'ko', sdiDate: '2026-11-03', sdiNote: 'codice fiscale del cliente non valido' },
    { n: 8, date: '2026-11-05', party: 'Hotel Lago Blu S.r.l.', imp: 1240.00, pay: 'Bonifico 30 gg', dues: [['2026-12-05', null]], paid: false, sdi: 'wait', sdiDate: '2026-11-05' },
    { n: 9, date: '2026-11-06', party: 'Studio Tecnico Ferri', imp: 455.00, pay: 'Bonifico 30 gg', dues: [['2026-12-06', null]], paid: false, sdi: 'gen' },
  ];
  for (const f of invoices) {
    f.iva = r2(f.imp * 0.22);
    f.total = r2(f.imp + f.iva);
    // il Comune è in scissione dei pagamenti: incassiamo il solo imponibile
    f.toCollect = f.pa ? f.imp : f.total;
    f.dues = f.dues.map(([d, a]) => ({ date: d, amount: a == null ? f.toCollect : a, paid: f.paid }));
    console.assert(r2(f.dues.reduce((s, d) => s + d.amount, 0)) === f.toCollect, 'rate = totale', f.n);
  }

  // fattura 6 nel dettaglio (stesse righe dell'archivio dimostrativo)
  const lines = [
    { kind: 'text', description: 'Rif. DDT n. 1 del 18/10/2026' },
    { code: 'SED01', description: 'Sedia ergonomica da ufficio', qty: 6, unit: 'pz', price: 149.00, disc: '10', vat: 22, cost: 82.00 },
    { code: 'SCR01', description: 'Scrivania 160x80 rovere', qty: 3, unit: 'pz', price: 320.00, disc: '10+5', vat: 22, cost: 190.00 },
    { code: 'MONT', description: 'Servizio di montaggio (ora)', qty: 4, unit: 'h', price: 38.00, disc: '', vat: 22, cost: 0 },
  ];
  for (const l of lines) {
    if (l.kind) continue;
    const net = l.disc ? l.disc.split('+').reduce((p, d) => r2(p * (1 - Number(d) / 100)), l.price) : l.price;
    l.amount = r2(net * l.qty);
  }
  const f6 = invoices.find((f) => f.n === 6);
  const linesImp = r2(lines.reduce((s, l) => s + (l.amount || 0), 0));
  console.assert(linesImp === f6.imp, 'righe = imponibile fattura 6', linesImp);
  const cost6 = r2(lines.reduce((s, l) => s + (l.cost || 0) * (l.qty || 0), 0));

  const received = [
    { party: 'Forniture Industriali S.r.l.', number: '2026/418', date: '2026-11-04', total: 483.85 },
    { party: 'Ufficio Pro S.p.A.', number: 'A-1192', date: '2026-11-06', total: 219.60 },
  ];
  const others = [
    { kind: 'Preventivo', label: 'Preventivo (bozza)', date: '2026-11-07', party: 'Mario Bianchi', total: 572.18, status: 'Bozza' },
    { kind: 'Vendita al banco', label: 'Vendita al banco 2', date: '2026-11-07', party: '', total: 39.60, status: 'Emessa' },
    { kind: 'Ordine cliente', label: 'Ordine cliente 1', date: '2026-11-07', party: 'Rossi Arredamenti S.p.A.', total: 2168.43, status: 'Da evadere' },
  ];
  const lowStock = [{ code: 'LAM01', name: 'Lampada LED da tavolo', qty: 2, min: 5 }];
  const inventory = { warehouse: 'Negozio', counted: 5, lines: 12 };

  const year = invoices.filter((f) => f.date.startsWith('2026'));
  const turnover = r2(year.reduce((s, f) => s + f.imp, 0));
  const openDues = invoices.flatMap((f) => f.dues.filter((d) => !d.paid).map((d) => ({ ...d, f })));
  const open = r2(openDues.reduce((s, d) => s + d.amount, 0));
  const overdue = openDues.filter((d) => d.date < TODAY);
  const overdueSum = r2(overdue.reduce((s, d) => s + d.amount, 0));
  const in30 = openDues.filter((d) => d.date >= TODAY && d.date <= '2026-12-07');
  const in30Sum = r2(in30.reduce((s, d) => s + d.amount, 0));
  console.assert(r2(overdueSum + in30Sum + openDues.filter((d) => d.date > '2026-12-07').reduce((s, d) => s + d.amount, 0)) === open, 'scadenze');

  const months = ['Giu', 'Lug', 'Ago', 'Set', 'Ott', 'Nov'];
  const byMonth = ['06', '07', '08', '09', '10', '11'].map((m) => r2(year.filter((f) => f.date.slice(5, 7) === m).reduce((s, f) => s + f.imp, 0)));
  console.assert(r2(byMonth.reduce((a, b) => a + b, 0)) === turnover, 'mesi = anno');

  // separatore delle migliaia anche a 4 cifre (4.756,70), come nei gestionali
  const fmtE = (v) => { const [i, dec] = Math.abs(v).toFixed(2).split('.'); return (v < 0 ? '−' : '') + i.replace(/\B(?=(\d{3})+(?!\d))/g, '.') + ',' + dec; };
  const fmtD = (iso) => iso.split('-').reverse().join('/');
  const daysBetween = (a, b) => Math.round((new Date(b) - new Date(a)) / 86400000);

  return { TODAY, invoices, lines, f6, cost6, received, others, lowStock, inventory, turnover, open, openDues, overdue, overdueSum, in30, in30Sum, months, byMonth, fmtE, fmtD, daysBetween };
})();
