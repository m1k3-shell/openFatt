# Richieste del 28/11/2026: analisi voce per voce

Ogni voce ha uno di quattro stati:
- **già presente**: c'era già prima di queste richieste;
- **completato**: c'era in parte, ora è finito;
- **nuovo**: fatto da zero;
- **fuori portata**: non si può fare da qui senza contratti, account o apparecchi; il motivo è indicato.

Le cifre e le regole fiscali usate vengono da fonti ufficiali o le può aggiornare l'utente:
- **Fondazione Enasarco 2025-2026**: aliquota, quota a carico dell'agente e massimali. La tabella si modifica per anno.
- **Circolare AdE 13/E del 2/7/2018**: reinvio dopo uno scarto entro 5 giorni.
- **Specifiche tecniche SdI 1.8.4**: la PA ha 15 giorni per l'esito.
- **FAQ AdE n. 45 del 21/12/2018**: estremi del documento commerciale nella fattura e scorporo dai corrispettivi. Il codice `N.DOC.COMM` (10 caratteri, come vuole lo schema) viene dalle specifiche tecniche.
- **Circolare AdE 44/E del 2012**: IVA per cassa anche sugli acquisti.

## P0

| Voce | Stato | Cosa c'è |
|---|---|---|
| Collaudo reale SdI/PEC, firma qualificata, home banking, RT fisici, e-commerce, NSO | fuori portata | Servono casella PEC, certificato qualificato, conto in banca, registratore e negozio reali. Al loro posto ci sono **prove virtuali**: <br>• ciclo SdI completo con le notifiche ufficiali di esempio (RC, NS, MC, NE); <br>• estratti conto camt.053 sintetici; <br>• XML validati con lo schema ufficiale; <br>• simulatore RT già presente. |
| Ciclo notifiche SdI guidato | completato | Pannello «Da gestire» con scadenza e azione: <br>• da inviare entro 12 giorni; <br>• nessun esito dopo 5 giorni; <br>• scarto: reinvio entro 5 giorni; <br>• mancata consegna: email di cortesia già pronta; <br>• PA: 15 giorni per l'esito; <br>• rifiuto PA: nota di credito che storna la fattura. |
| Conservazione a norma | completato | Responsabile, delegato e conservatore nell'indice del pacchetto; bozza del manuale; verifica delle impronte di un pacchetto salvato. L'integrazione via API con un conservatore accreditato e la marca temporale (servizio TSA a pagamento) sono **fuori portata**. |
| Multiutente, migrazione, disaster recovery | completato | **Già presenti**: copie verificate e ripristino. **Nuovi**: <br>• «Trasferisci su un altro computer»: copia verificata dell'archivio, impostazioni delle copie senza password e istruzioni; <br>• «Importa impostazioni» sul computer nuovo. <br>Non è stato provato su una vera rete. |

## P1

| Voce | Stato | Cosa c'è |
|---|---|---|
| Regime forfettario | completato | Per l'azienda in RF19: N2.2 nelle nuove vendite, bollo automatico, nessuna ritenuta, dicitura del comma 67 sulle parcelle. |
| IVA per cassa | completato | Esigibilità «D» sulle vendite verso clienti con partita IVA (verso i privati resta immediata, art. 32-bis c. 1) e sugli acquisti, con la dicitura dell'art. 32-bis. Vale per ogni documento creato dall'archivio (trasformazioni, differite, ricorrenti, fatture ricevute); la scissione dei pagamenti prevale. La scadenza annuale era già presente. |
| Pro-rata, prospetto annuale | già presente | |
| Regime del margine, fuori campo | già presente | N5 e spese fuori campo in prima nota. Le contabilità separate restano da valutare con il commercialista. |
| Seconda ritenuta Enasarco con massimali | nuovo | RT04 con il residuo del massimale per preponente; mono o plurimandatario; società di capitali senza massimale. |
| Ritenute per riga e per cliente, riepiloghi | completato | La ritenuta per riga c'era già. **Nuovi**: profilo di ritenute nella scheda del cliente e riepilogo per percipiente nell'anno del pagamento. Il file telematico CU/770 è **fuori portata**: lo prepara il commercialista con questi dati. |
| Piano dei conti, transcodifica, giroconti, saldi iniziali, riconciliazione, F24 | nuovo | Esportazione CSV con i codici dello studio. I tracciati proprietari dei programmi per commercialisti sono **fuori portata**. |
| Dati di trasporto | completato | Aggiunto il numero di spedizione in stampa. I connettori con i corrieri sono **fuori portata** (API con contratto). |
| Omaggi | già presente | Sconto merce al 100% e autofattura TD27. |
| CONAI ed eco-contributi | già presente | Il contributo ambientale per articolo ha un'etichetta libera, quindi vale anche per il CONAI. |
| Fatture di sola IVA | da verificare | Non è stata trovata una regola ufficiale per una TD01 con imponibile zero: non è stata fatta per non produrre file a rischio di scarto. |
| Riordino sottoscorta | nuovo | |
| Terminalini barcode | già presente | Importazione dell'inventario da file. |
| Codice fiscale e lotteria sullo scontrino | già presente | |
| Scontrino → fattura | nuovo | |
| App per agenti, conferme d'ordine | fuori portata | L'app mobile è fuori portata. L'importazione delle conferme d'ordine va definita sul formato dei fornitori reali. |
| Sedi tipizzate, referenti con ruoli, fido e blocco | nuovo | Per i referenti si sono aggiunti i ruoli proposti. |
| Importazione anagrafiche da file | già presente | CSV ed Easyfatt-XML. |

## P2

| Voce | Stato | Cosa c'è |
|---|---|---|
| Stampe termiche, copie e originali | nuovo | Rotolo da 80 e 58 mm (PDF per il driver della stampante); copie con la loro dicitura. |
| Dashboard personalizzabili | già presente | Dalla v0.1.10. |
| Analisi ABC, commesse, report programmati | nuovo | |
| Archivio pulito, migrazione della postazione | nuovo | |
| WhatsApp | nuovo | Collegamento pubblico wa.me con il messaggio pronto; il PDF si trascina nella chat. Nessuna API. |
| Connettori corrieri ed e-commerce aggiuntivi | fuori portata | API proprietarie con contratto. WooCommerce e Shopify erano già presenti. |

## Verifiche

**Test automatici**: 263, tra cui:
- ciclo SdI simulato;
- riconciliazione da camt.053;
- Enasarco con massimale;
- scontrino → fattura con liquidazione invariata;
- F24;
- archivio pulito;
- riordino;
- ABC e commesse;
- rotolo e copie.

**Collaudo completo** con le nuove funzioni; tutti gli XML prodotti sono validi contro lo schema FatturaPA 1.2.3.

**Catture dello schermo** delle nuove finestre:
- riconciliazione;
- F24;
- piano dei conti;
- riordino;
- ritenute ed Enasarco;
- SdI;
- banco;
- statistiche ABC;
- fine anno.
