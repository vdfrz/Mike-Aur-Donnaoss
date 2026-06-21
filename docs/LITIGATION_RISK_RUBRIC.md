# Litigation Drafting & Review Risk Rubric — Donna / Mike

**Purpose.** A litigation-first risk rubric injected into the backend system prompt so the desktop app and the Telegram bot both (a) flag risks *while drafting* and (b) *redline/review* documents a lawyer uploads. Jurisdiction: India (Delhi District Courts, Delhi HC, Supreme Court, ITAT, AFT, GSTAT/VAT tribunals, CIC, J&K).

**Scope split.** ~85% of work is **litigation / pleadings** (PRIMARY). ~15% is **transactional** (SECONDARY — condensed in §C). This rubric does **not** repeat the statute-transition table (IPC/CrPC/IEA → BNS/BNSS/BSA), the Kanoon citation-verification workflow, or the verification-split / supporting-affidavit hygiene rules — those already live in `MIKE_SYSTEM_PROMPT` (`src/routes/chat.rs`). This rubric *layers the cross-cutting risk-spotting* on top of them.

**Grounded on real exemplars** (read while building this):
- `clean data/income_tax/ITAT Appeal.odt.txt` — second appeal anatomy; condonation as alternate prayer; clean-hands boilerplate; alternative pleading; "craves leave to add grounds".
- `clean data/income_tax/affidavit condonation.docx.txt` — condonation application + affidavit + verification on oath; §249(3) IT Act power to condone; limitation last-date computation.
- `clean data/property_real_estate/Application Under 7 Rule 11.pdf.txt` — O7R11 reply; "only the plaint is seen" rule; para-wise denial discipline; *Dahiben v. Arvindbhai* (2020) 7 SCC 366; unsigned-application objection.
- `clean data/rti_information/subhash v CIC.pdf.txt` — CIC second appeal; statutory timelines (first appeal → second appeal u/s 19(3)); §8(1)(a)/§8(2) exemptions.
- `clean data/family_matrimonial/REPLY ON BEHALF OF RESPONDENT.docx.txt` — DV Act §12 reply; locus, clean-hands, "held to strict proof", para-wise traverse, suppression.
- `clean data/consumer/CHARANJEET W.S OPPOSITE PARTY.pdf.txt` — consumer WS; **wrong-Act objection (1986 vs 2019)**, jurisdiction bar, limitation/delay, unclean hands.
- `clean data/negotiable_instruments/(PHC) RAJ KUMAR SINGH V. KAMLESH & SUNIL KUMAR.pdf.txt` — Order XXXVII summary suit; index of annexures (legal notice, cheque, return memo, vakalatnama, court-fee, exemption); memo of parties.
- `clean data/armed_forces_service/notice.odt.txt` — **S.80 CPC notice** to government; 30-day demand; annexure cross-refs.
- `clean data/armed_forces_service/VINOD KUMAR AFT OA.pdf.txt` — AFT OA under §14 AFT Act; "List of Dates & Events"; government parties arrayed "Union of India through Secretary"; condonation + exemption + vakalatnama bundle.
- `clean data/labour_employment/RANJU DEVI W.S ON THE BEHALF RESPONDENT NO 1.pdf.txt` — WS opening; general denial → preliminary objections → para-wise reply; "want of specific traverse" / evasive-denial discipline.
- `clean data/indirect_tax_gst/Memorandum of Appeal.docx.txt` — tribunal appeal; impugned-order-received date drives limitation; verification by authorised signatory; statement of facts → grounds → prayer.

---

## How to read the rubric

Every issue carries a **severity** and a **side**:

- **HIGH** = can get the pleading rejected/dismissed, time-barred, or struck — a filing-killer. Flag even if the user didn't ask.
- **MED** = weakens the case, invites a successful preliminary objection, or costs an amendment.
- **LOW** = polish, hygiene, persuasiveness.
- **Side** = whether catching/raising it **helps the party we represent** or **hurts them** (i.e. the opponent can exploit it). When the user hasn't said which side they're on, **ask once** before redlining — the same defect is a sword for one side and a wound for the other.

---

# §A — CROSS-CUTTING LITIGATION RISKS

These apply to almost every pleading. For an **upload/review**, run this whole list as a checklist. For **drafting**, surface the HIGH items proactively.

### A1. LIMITATION (Limitation Act, 1963)
| | |
|---|---|
| **Severity** | **HIGH** — a time-barred suit/appeal is liable to dismissal even if not pleaded (s.3 — court bound to dismiss *suo motu*). |
| **Look for** | No date the cause of action arose; appeal/petition filed beyond the article's period; "received the impugned order on ___" missing (the limitation clock for appeals runs from *receipt/knowledge*, not the order date — see GST memo and ITAT exemplars); continuing wrong vs one-time wrong confusion; acknowledgment (s.18) / part-payment (s.19) that resets the clock not pleaded; no condonation application despite obvious delay. |
| **Computing** | Identify the **Article** (e.g. Art 54 — possession on title 12 yrs; Art 113 — residuary 3 yrs; Art 116/117 — appeals 90/30 days; cheque-bounce complaint window below). Exclude the day of the event (s.12), exclude time spent bona fide in a wrong forum (s.14), exclude time obtaining certified copy. State the **last date** expressly. |
| **Fix / redline** | Insert "The cause of action arose on ___ and the present [suit/appeal] is within limitation under Article ___ of the Limitation Act, 1963." If late, draft a **separate condonation application + supporting affidavit** (s.5; for IT appeals s.249(3); show "sufficient cause", non-deliberate, day-by-day for long delays per *Collector, Land Acquisition v. Katiji*). Never bury condonation as a one-line prayer at the foot of the main pleading without an affidavit. |
| **Side** | Helps **respondent/defendant** (raise as preliminary objection). Hurts **plaintiff/appellant** if missed. |

### A2. JURISDICTION (territorial / pecuniary / subject-matter)
| | |
|---|---|
| **Severity** | **HIGH** — wrong forum = return of plaint (O7R10) or dismissal; subject-matter defect can void the decree. |
| **Look for** | Suit filed where neither the cause of action arose nor the defendant resides (s.20 CPC); value exceeds the court's pecuniary ceiling or is pitched to dodge it; **wrong statute/forum entirely** (consumer exemplar: complaint filed under the *repealed* Consumer Protection Act 1986 instead of the 2019 Act — fatal); tribunal's exclusive jurisdiction ousting the civil court (AFT, ITAT, GSTAT, DRT, NCLT, RERA, Labour/EC Commissioner); §9 CPC bar; ouster clauses; arbitration clause ousting the court (s.8 A&C reference). |
| **Fix / redline** | Add a "Jurisdiction" paragraph tying *which part of the cause of action arose within this court's territory* and confirming the pecuniary value falls within limits. If wrong forum, advise transfer/return and re-file; do not paper over it. |
| **Side** | A jurisdiction bar is the defendant's first and cheapest win. Helps **defendant/OP**; hurts **plaintiff**. |

### A3. CAUSE OF ACTION
| | |
|---|---|
| **Severity** | **HIGH** — no/defective cause of action → plaint rejected under **Order VII Rule 11(a)** (see O7R11 exemplar). |
| **Look for** | Bundle of facts that gives the right to sue not pleaded; *when* it arose unstated; relief sought that doesn't flow from the pleaded facts; clever drafting/"illusory cause of action" to avoid the bar (courts pierce this — *Dahiben*, *T. Arivandandam*). On review of an O7R11 application: remember **only the plaint** is looked at, not the WS/defendant's documents. |
| **Fix / redline** | Plead the operative facts and the date each arose. For a reply to O7R11, lead with "only the plaint is to be seen" and show the cause of action is disclosed on the face of the plaint. |
| **Side** | Sword for **defendant** (O7R11), wound for **plaintiff**. |

### A4. COURT FEE & VALUATION
| | |
|---|---|
| **Severity** | **MED→HIGH** — deficient court fee can stall registration; gross under-valuation invites O7R11(b)/(c) rejection if not cured. |
| **Look for** | Ad valorem fee not computed on the relief's value; declaratory + consequential relief fee mismatch; "Valuation of Suit / Court Fee paid" blank (NI exemplar's case-info form leaves these to fill); fixed-fee used where ad valorem is required. |
| **Fix / redline** | State valuation for jurisdiction and for court fee separately, and the fee actually affixed. Flag where the relief mix (declaration + injunction + possession) changes the fee head. |
| **Side** | Neutral hygiene; a defendant can exploit gross deficiency. |

### A5. VERIFICATION, OATH & AFFIDAVIT DEFECTS
| | |
|---|---|
| **Severity** | **HIGH** — an unverified/defectively-verified pleading or an unsworn affidavit is liable to be ignored or struck. |
| **Look for** | Missing verification clause (Order VI Rule 15 CPC); verification not split into *true to knowledge* vs *true to information & belief* (already a house rule — enforce it); no place/date; affidavit not affirmed before an oath commissioner/notary; "para-wise" verification absent; affidavit deponent lacks competence/authority (authorised-signatory/board-resolution for a company — GST memo). For pleadings that plead facts, the **supporting affidavit under s.26(2) CPC** must accompany. |
| **Fix / redline** | Append/repair the verification clause; split knowledge vs belief; add place, date, deponent capacity; ensure the affidavit is sworn and identifies the deponent's authority. (Mechanics already covered in the base prompt — this entry exists so review flags the *absence*.) |
| **Side** | Both — your own defect is fatal; the opponent's defective verification is a point to take. |

### A6. MATERIAL FACTS vs EVIDENCE & SUPPRESSION
| | |
|---|---|
| **Severity** | **HIGH** (writs especially) — suppression of material facts defeats a writ outright (unclean hands); pleading evidence not facts invites strike-out. |
| **Look for** | Pleadings that narrate *evidence* (witness statements, documents verbatim) instead of *material facts* (O6R2 — "facts not evidence"); facts deliberately omitted that change the equities; in writs, non-disclosure of a parallel remedy already availed, an adverse earlier order, or delay. The clean-hands/"no suppression" boilerplate appears in the ITAT, DV-reply and consumer exemplars — but it must be *true*, not decorative. |
| **Fix / redline** | Strip evidentiary matter to "material facts"; surface anything adverse (better disclosed by you than weaponised by the opponent); for writs, an affidavit that all material facts are disclosed. |
| **Side** | Suppression hurts the **petitioner/plaintiff**; raising the opponent's suppression helps the **respondent**. |

### A7. PARTIES — JOINDER, CAPACITY, REPRESENTATION
| | |
|---|---|
| **Severity** | **HIGH** — non-joinder of a *necessary* party can dismiss the suit; suing in the wrong capacity is fatal. |
| **Look for** | Necessary party omitted (e.g. all co-owners in a partition; the company itself when its acts are challenged); mis-joinder of unnecessary parties (O1R9/O1R10); suing/defending in proper capacity (karta, guardian for a minor — O32, legal heirs on death — O22 abatement, partnership in the firm name — O30); **government correctly arrayed** ("Union of India through Secretary, Ministry of ___" — AFT exemplar) and the right authority impleaded; representative suit requirements (O1R8) and court permission/notice. On review: is anyone *un-arrayed* against whom relief is sought? |
| **Fix / redline** | Complete the memo of parties with correct capacities and addresses; add necessary parties; for a deceased party, move O22 substitution before abatement; for government, name the correct office-holder. |
| **Side** | Both — non-joinder is a defendant's objection; correct arraying protects the plaintiff. |

### A8. STATUTORY PRE-CONDITIONS (notices, mediation, registration bars)
| | |
|---|---|
| **Severity** | **HIGH** — a missing mandatory pre-condition makes the action premature/barred regardless of merits. |
| **Look for & enforce the clock:** |
| **S.80 CPC** — 2-month notice to government before suit (unless s.80(2) urgent-relief leave) — see S.80 notice exemplar. |
| **S.12A Commercial Courts Act** — pre-institution mediation mandatory in commercial suits with no urgent interim relief (*Patil Automation*). |
| **S.138/142 NI Act** — (i) cheque presented within validity; (ii) **demand notice within 30 days** of the bank's return memo; (iii) drawer gets **15 days** to pay; (iv) complaint filed **within 1 month** after the 15-day window expires (s.142(b)); payee/holder-in-due-course alone has locus. |
| **S.69 Partnership Act** — unregistered firm barred from suing to enforce a contractual right. |
| **S.21 A&C Act** — arbitration commences only on the s.21 notice; affects limitation and s.11 reference. |
| **Other** — RERA/consumer pre-deposit for appeals; s.18 SARFAESI deposit; statutory notice under specific rent/eviction laws; election-petition/service-rules representations (AFT: representation before OA). |
| **Fix / redline** | Plead the pre-condition was satisfied, with the **date and annexure** of the notice; if not yet done, advise issuing it before filing, or pleading the urgency exemption. |
| **Side** | Sword for **defendant/respondent** (premature-suit objection); the **plaintiff** must pre-empt it. |

### A9. PRAYER / RELIEF DEFECTS
| | |
|---|---|
| **Severity** | **MED→HIGH** — relief not flowing from facts isn't grantable; omnibus prayers get trimmed; un-sought interim relief is lost. |
| **Look for** | Relief that doesn't trace to a pleaded fact; declaration without the **consequential relief** (possession/injunction) — a bare declaration may be refused (s.34 SRA proviso); no specific *and* alternative relief; interim relief not separately prayed; "any other relief" without a concrete primary prayer; in tax/tribunal, no alternate prayer (the ITAT exemplar prays *quash, or in the alternate re-assess / apply presumptive tax* — model that). |
| **Fix / redline** | Make each prayer trace to a fact and a legal basis; add the consequential relief; add an explicit alternative; add a separate interim-relief prayer; keep the residuary "such other relief as the Hon'ble Court deems fit" *as a tail, not the spine*. |
| **Side** | Mostly helps the **party seeking relief** (don't leave relief on the table). |

### A10. INTERIM RELIEF (injunctions, stay, status quo)
| | |
|---|---|
| **Severity** | **HIGH** when interim relief is the point of the filing. |
| **Look for** | The **three-fold test** not pleaded — (i) *prima facie* case, (ii) **balance of convenience**, (iii) **irreparable injury**; no undertaking as to damages; ex-parte ad-interim sought without satisfying O39R3 (notice/urgency reasons recorded) and the *Morgan Stanley* safeguards; no proximate urgency; status-quo/stay prayer vague (status quo *as to what, as of when?*). |
| **Fix / redline** | Plead all three limbs separately and tie each to facts; justify ex-parte under O39R3 with reasons; add the damages undertaking; define the status quo precisely. |
| **Side** | Helps the **applicant**; a respondent attacks any missing limb. |

### A11. DENIAL DISCIPLINE (Written Statements / Replies)
| | |
|---|---|
| **Severity** | **HIGH** — an **evasive denial is deemed an admission** (Order VIII Rules 3–5); a vague general denial doesn't traverse. |
| **Look for** | "All allegations are denied" with no para-wise traverse; failure to specifically deny a material allegation (deemed admitted); the WS exemplars' correct pattern — general denial **plus** preliminary objections **plus** numbered para-wise reply, each putting the claimant "to strict proof" (DV-reply, consumer WS, labour WS); facts within the defendant's knowledge not specifically dealt with. |
| **Fix / redline** | Convert blanket denials into **specific, para-wise** denials; for every allegation either admit, deny, or state "not admitted, claimant put to strict proof"; never leave a material averment untraversed. |
| **Side** | Critical for the **defendant/respondent**; the **plaintiff** exploits evasive denials as admissions. |

### A12. ALTERNATIVE & INCONSISTENT PLEADING
| | |
|---|---|
| **Severity** | **MED**. |
| **Look for** | Failure to plead in the alternative where the law allows (the ITAT exemplar's "quash, *or in the alternate* re-assess" is the model); mutually destructive pleadings that aren't framed as alternatives; approbation-and-reprobation. |
| **Fix / redline** | Frame fallback positions as express alternatives ("without prejudice to the above, and in the alternative…"). |
| **Side** | Helps the pleader by preserving fallbacks. |

### A13. SET-OFF & COUNTERCLAIM
| | |
|---|---|
| **Severity** | **MED** — a counterclaim not raised may be barred later (O8R6A); legal set-off has form/court-fee requirements. |
| **Look for** | Ascertained sum due to the defendant not pleaded as legal set-off (O8R6); independent claim not pleaded as counterclaim though it arose before the WS; counterclaim's own limitation/court-fee not satisfied. |
| **Fix / redline** | Add set-off/counterclaim with its own valuation and court fee; confirm it's within limitation as on the date of the WS. |
| **Side** | Helps the **defendant**. |

### A14. CROSS-REFERENCE, RENUMBERING & ANNEXURE INTEGRITY
| | |
|---|---|
| **Severity** | **MED** — broken cross-refs and orphan/uncited annexures are an immediate credibility hit and a hook for the opponent. |
| **Look for** | A "List of Dates & Events" (AFT/GST exemplars) inconsistent with the body; annexures listed in the index but never cited in the body, or cited but missing; paragraph numbers referenced internally that don't exist after edits; "Annexure A-__" placeholders left blank; the index page-numbers not matching. |
| **Fix / redline** | Cite each annexure in the body **exactly once**; never list an uncited annexure; renumber paragraphs and fix every internal "para __ above" after edits; reconcile the index, List of Dates, and body. |
| **Side** | Hygiene — protects the filer's credibility. |

### A15. SIGNATURE, VAKALATNAMA, COURT-FEE & LIMITATION ENDORSEMENTS
| | |
|---|---|
| **Severity** | **MED→HIGH** — an unsigned pleading/application is liable to be dismissed (O7R11 reply exemplar took the point that "the application is not signed by the defendants"). |
| **Look for** | Pleading not signed by party **and** counsel; missing/blank vakalatnama; no exemption application where required; court-fee receipt absent; limitation endorsement / certificate of fresh filing missing; in appeals, **certified copy of the impugned order** not annexed. |
| **Fix / redline** | Ensure signatures of party and advocate, vakalatnama, court-fee, exemption application, and certified copy of the impugned order are in the bundle; flag any missing item in the index. |
| **Side** | Both. |

### A16. STATUTE TRANSITION & CITATION HYGIENE (pointer)
The **IPC/CrPC/IEA → BNS/BNSS/BSA** transition (offences before vs on/after **1 July 2024**), the "don't mix codes in one document" rule, the **never-fabricate-citations** rule, and the **kanoon_search → kanoon_verify_case** workflow are already mandated in the base system prompt. The rubric's only addition: **when reviewing an uploaded pleading, flag any section cited under the wrong code for its offence date, any case cited without a pinpoint/that you cannot verify, and any *per incuriam*/overruled authority** — distinguish binding (SC, jurisdictional HC) from merely persuasive.

---

# §B — DOCUMENT-TYPE-SPECIFIC CHECKLISTS

Only the *risky bits unique to each type* — the cross-cutting §A list still applies to all.

### B1. Plaint / Civil Suit
- Cause of action + its date (A3); jurisdiction + valuation paragraph (A2/A4); limitation paragraph (A1); necessary parties (A7); **pre-conditions** (s.80 CPC / s.12A CC Act — A8); specific + alternative + interim prayers (A9/A10); verification + supporting affidavit (A5); list of documents (O7R14 — documents relied on must be filed or leave sought). Summary suit under **Order XXXVII** (NI exemplar): plead it's a liquidated demand on a written contract/instrument, and that the plaint contains the O37 averment that the suit is filed under the summary procedure and no relief outside O37 is claimed.

### B2. Written Statement
- **Denial discipline** (A11) is the whole ballgame: general denial + preliminary objections + para-wise reply, each averment admitted/denied/"put to strict proof". File **within 30 days** (extendable to 90; for commercial suits the **120-day outer limit is mandatory** — beyond it the right to file WS is forfeited, *SCG Contracts*). Take all preliminary objections (limitation, jurisdiction, non-joinder, cause of action, maintainability, suppression). Plead **set-off/counterclaim** now (A13). Don't admit by silence.

### B3. Replication / Rejoinder
- New pleas not permitted to set up a new case; confine to meeting the WS's new facts. Don't introduce a fresh cause of action. Verify like a pleading.

### B4. Writ Petition (Art 226 / 32)
- **Alternative remedy** — explain why the writ lies despite an available statutory remedy (breach of natural justice / lack of jurisdiction / vires challenge / fundamental-rights breach). **Delay & laches** — explain any delay (no fixed limitation but laches defeats). **Locus standi** — petitioner's standing (or PIL bona fides). **Suppression / clean hands** (A6) — a single material suppression sinks the writ; verify on affidavit that all material facts are disclosed. Array the correct State/authority respondents. Relief: certiorari/mandamus/prohibition framed precisely; interim stay with the three-fold test (A10). RTI/CIC second appeals (CIC exemplar): plead the first-appeal date and the s.19(3) timeline.

### B5. Criminal Complaint & S.138 NI Act
- **S.138 mandatory chain** (A8): dishonour memo → **30-day** demand notice → **15-day** payment window → complaint **within 1 month** of that window's expiry (s.142(b)); only **payee/holder in due course** has locus; plead presentment within cheque validity. **Vicarious liability of directors (s.141)** — for a company, plead the company as accused **and** that each director/signatory **was in charge of and responsible to the company for the conduct of its business** at the relevant time (*S.M.S. Pharmaceuticals*, *Pooja Ravinder Devidasani* — a mere director isn't automatically liable; the signatory and the company always are). General criminal complaint: array **every** wrongdoer and entity (already a house rule), tie each forged instrument to its maker, use placeholders for unknown accused.

### B6. Bail / Anticipatory Bail (BNSS §480/§483; §482 anticipatory)
- State the FIR/case number, sections, custody status, and stage. Address the **triple test**: flight risk, tampering with evidence, influencing witnesses; plus gravity, role attributed, antecedents, parity with co-accused. Anticipatory bail (§482 BNSS / former §438 CrPC) — apprehension of arrest in a *cognizable, non-bailable* offence; offer to cooperate; for serious special-statute offences flag the statutory rigour (UAPA/NDPS s.37/PMLA twin conditions). Don't over-plead facts that concede guilt.

### B7. Appeal / Revision (civil, ITAT, AFT, GSTAT, CIC)
- **Limitation from the date of receipt of the certified/impugned order** (A1; GST and ITAT exemplars compute from "received on ___"); annex the **certified copy**; **condonation** application + affidavit if late (A1). **Grounds of appeal** — concise, numbered, each a distinct error of law/fact (ITAT exemplar); a "the appellant craves leave to add/alter grounds" tail. **Statement of facts / List of Dates** consistent with grounds. Pre-deposit where the statute requires it (tax/RERA/consumer). Don't argue evidence in the grounds — state the error.

### B8. Affidavit (evidence / service / condonation / income)
- Competent deponent with stated capacity and knowledge; numbered paragraphs; **knowledge vs information-&-belief split** (house rule); sworn/affirmed before the proper authority; place + date; exhibits referred to and marked. Evidence affidavit (O18R4) confined to admissible, pleaded facts. Service affidavit states mode and date of service. Condonation affidavit swears to the delay reasons (condonation exemplar).

### B9. Legal / Demand Notice
- A notice is **not a pleading** — no verification clause; it takes a sender/authority block and is *issued on instructions of the client*. Plead the demand, the **time to comply** (the S.80 notice exemplar gives 30 days; the s.138 notice **must** give 15 days and issue within 30 days of dishonour — A8), the consequence of non-compliance, and keep an office copy. For government, the s.80 CPC 2-month clock; for cheque bounce, the strict s.138 timeline.

### B10. Arbitration — S.9 / S.11 / S.34
- **S.9** (interim measures) — three-fold injunction test (A10); can be pre/during/post-award. **S.11** (appointment) — confirm the s.21 notice was issued, the limitation for reference, and existence of an arbitration agreement; post-2015/2019 amendments and *In re Interplay*/stamping issues flagged. **S.34** (set-aside) — **strict limitation: 3 months + 30 days condonable, no more** (*P. Radha Bai*); grounds confined to s.34(2)/(2A) (patent illegality for domestic awards, public policy, no review on merits); file within time or it's dead.

### B11. Consumer Complaint
- **Limitation 2 years** from the cause of action (s.69 CP Act 2019), with reasoned condonation if late. **Pecuniary jurisdiction** under the **2019 Act** thresholds (District ≤ ₹50 lakh / State ≤ ₹2 cr / National > ₹2 cr — value of *goods/services paid as consideration*, not compensation claimed). **File under the 2019 Act, not the repealed 1986 Act** (the consumer WS exemplar makes this the lead objection — a fatal, common error). Establish "**consumer**" status (s.2(7) — not for commercial purpose). Territorial jurisdiction now includes where the complainant resides/works.

### B12. Matrimonial / Family
- Jurisdiction under the governing Act (HMA s.19 / Special Marriage Act / DV Act §27); plead the marriage, its date and rites, and residence; for DV Act §12 (DV-reply exemplar) — "aggrieved person" + "domestic relationship" + "shared household"; cruelty/desertion grounds pleaded with particulars (dates, incidents) not bald allegations; maintenance claims plead income/needs (*Rajnesh v. Neha* affidavit of assets). Reliefs (§§18–23 DV Act) each tied to facts.

### B13. Eviction / Rent / Property
- Plead the **relationship** (landlord-tenant / licensor-licensee — which statute governs: Rent Control vs Transfer of Property Act s.106 notice), the **statutory ground** (bona fide need, default, sub-letting), and the **notice to quit** where required (s.106 TPA / Rent Act notice — a pre-condition, A8). For partition (O7R11 exemplar) — array **all co-sharers** (A7), plead the devolution/title, and that no prior partition exists. Possession suits plead the plaintiff's title/possession and the defendant's unlawful entry, with valuation on market value (A4).

---

# §C — TRANSACTIONAL / CONTRACT RISK CHECKLIST

Apply when **drafting or reviewing a contract, deed, or commercial instrument** (~15% of work, but the most expensive single clauses live here). This is the §A/§B litigation list's sibling — it does **not** repeat limitation, jurisdiction-as-forum, verification, or pleading rules from §A; it is the clause-by-clause risk map for the *instrument itself*. Same discipline: tag every issue **HIGH / MED / LOW**, say **which side** the clause favours and which it wounds, and **ask once which party we draft for** before redlining — an indemnity is a shield for one side and a noose for the other.

**Read first, then redline.** For a review, walk all 20 heads as a checklist and the cross-cutting India sweep (§C-X) at the end; for drafting, build the protective version and flag the HIGH heads you resolved. Output the same triage table (Clause | Severity | Side | Issue | Redline) before the marked-up clauses.

### C1. INDEMNITY — scope, triggers, control
| | |
|---|---|
| **Severity** | **HIGH** — an uncapped, third-party-and-direct, "hold harmless against all claims" indemnity is the single most dangerous clause in most contracts; it can dwarf the contract value. |
| **Why risky** | A standalone indemnity creates a **debt/contractual claim** that can bypass the proof-of-loss, mitigation, remoteness and limitation rules that govern an ordinary §73 damages claim. One-sided indemnities (only the customer/buyer is indemnified) shift the entire risk of the deal. |
| **Look for** | No **monetary cap** on the indemnity (or it is carved *out* of the liability cap — see C17); covers indemnitee's **own negligence**; no requirement of **notice, sole/joint conduct of defence, mitigation, no-admission-without-consent**; "indemnify against all claims **whether or not** arising from breach"; gross-up/tax clauses inflating the payout; indemnity survives termination indefinitely; first-party (direct-loss) indemnity used to dress up an ordinary damages claim and dodge the cap. |
| **Redline / fix** | Tie indemnity to **defined trigger events** (third-party IP claim, breach of confidentiality, statutory non-compliance), not "any breach"; **cap it** (inside or expressly outside the C2 cap — decide consciously); add notice + defence-control + mitigation + no-settlement-without-consent; exclude the indemnitee's own default; make it **mutual** where the bargain warrants. |
| **India hook** | **§§124–125 Contract Act** (contract of indemnity = promise to save from loss caused by the promisor or a third person; indemnity-holder's rights to recover damages, costs and sums paid in a suit). Indian courts also enforce indemnities on a **"loss-suffered/liability-accrued"** basis — confirm whether payment triggers on actual loss or on accrued liability. |

### C2. LIMITATION / EXCLUSION OF LIABILITY — caps and carve-outs
| | |
|---|---|
| **Severity** | **HIGH** — the cap decides who eats a catastrophic loss. |
| **Why risky** | A liability cap (e.g. "aggregate liability ≤ fees paid in the last 12 months") plus a blanket exclusion of **indirect/consequential/loss-of-profit** damages can leave the innocent party with almost no remedy. One-sided caps (protecting only the supplier) are common and easy to miss. |
| **Look for** | No **aggregate cap**, or a cap so low it is illusory; consequential-loss exclusion that silently swallows the buyer's **real** loss (in India "consequential" is read narrowly — say what is excluded); **no carve-outs** from the cap/exclusion for fraud, wilful misconduct, death/personal injury, IP infringement, breach of confidentiality, or indemnity obligations; exclusion of **all** implied terms; cap that purports to exclude liability for **fraud** (unenforceable). |
| **Redline / fix** | Insert an **aggregate cap** sized to the deal (super-cap for indemnity/IP/confidentiality); **carve out fraud, wilful default, IP, confidentiality, statutory and indemnity liabilities** from both the cap and the consequential-loss exclusion; make the cap **mutual**; define "consequential loss" precisely rather than relying on the label. |
| **India hook** | Freedom of contract is respected, **but a clause cannot oust §73 liability for fraud or exclude liability that public policy (§23) forbids**; an unconscionable exclusion in a standard-form/unequal-bargaining contract can be struck (*Central Inland Water Transport v. Brojo Nath Ganguly*). |

### C3. LIQUIDATED DAMAGES / PENALTY / FORFEITURE
| | |
|---|---|
| **Severity** | **HIGH** — over-pitched LD clauses are routinely cut down; under-pitched ones cap your client's recovery. |
| **Why risky** | India does **not** enforce penalties. A stipulated sum (or forfeiture of deposit/advance) recoverable on breach is capped at **reasonable compensation actually proved** — and the named figure is the **ceiling, not a floor**. A party relying on a large LD number without proving loss can recover little; a party who agreed to a penal forfeiture can claw most of it back. |
| **Look for** | LD pitched as a round, deterrent "penalty" untethered to a genuine pre-estimate; **forfeiture of advance/earnest/security** on default; "time is the essence" + forfeiture combined; LD claimed **without any proof of loss**; LD that is the *sole and exclusive remedy* (may bar the larger real claim); a "minimum guarantee/take-or-pay" that is really a disguised penalty. |
| **Redline / fix** | Frame the figure as a **genuine pre-estimate of likely loss** with a short recital of how it was computed; keep earnest/forfeiture **reasonable and proportionate**; preserve the right to **prove actual loss up to the cap**; for the paying side, recharacterise a penalty as such and reserve the §74 defence. |
| **India hook** | **§74 Contract Act** + **§75** (compensation on rightful rescission). *Fateh Chand v. Balkishan Das*; *Maula Bux*; **\*Kailash Nath Associates v. DDA\*** (no automatic forfeiture; must prove loss; named sum is the upper limit); *Construction & Design Services v. DDA* (genuine pre-estimate excuses proof of exact loss). Earnest/security forfeiture is tested the same way. |

### C4. TERMINATION ASYMMETRY & CURE RIGHTS
| | |
|---|---|
| **Severity** | **HIGH** when termination = loss of the business/investment; otherwise MED. |
| **Why risky** | One side gets **termination for convenience on short notice** while the other is locked in; or a **cure period** is given to one party only; or "material breach" is undefined so any minor lapse triggers termination. |
| **Look for** | Convenience-termination for one side only; **no cure period**, or cure for the strong party only; "material breach" undefined; termination on **insolvency/change-of-control** one-directional; immediate termination with no wind-down/transition; consequences of termination (refunds, transition assistance, data return) silent or one-sided; ipso-facto termination on insolvency (now constrained by IBC moratorium for corporate debtors). |
| **Redline / fix** | Make convenience-termination and notice **symmetrical** or price the asymmetry; add a **reciprocal cure period** (e.g. 30 days written notice to remedy); define "material breach"; add **transition assistance + data return + pro-rata refund**; align insolvency termination with IBC reality. |
| **India hook** | Specific performance of a determinable/terminable contract is generally **not** granted (Specific Relief Act §14); but post-2018 SRA amendments make specific performance the norm for performable contracts — check whether termination converts the remedy to damages only. |

### C5. LOCK-IN & EARLY-EXIT PENALTIES
| | |
|---|---|
| **Severity** | **MED→HIGH** (HIGH in leases, financing, SaaS, distribution). |
| **Why risky** | A lock-in plus a steep early-exit charge is a **penalty/restraint** dressed as a commercial term; it traps the weaker party and the exit fee may be unenforceable. |
| **Look for** | Long lock-in with **forfeiture of deposit/all paid fees** on early exit; exit fee unrelated to actual loss; lock-in on **only one** party; lock-in in an employment/services contract used to deter resignation (bond). |
| **Redline / fix** | Make lock-in mutual or compensated; size any exit charge to **genuine pre-estimate** (C3/§74); for employment bonds, limit recovery to **actual, proved training cost** (penal bonds are read down). |
| **India hook** | **§74** (exit fee as penalty) and **§27** (an employment lock-in/bond that operates as a restraint on taking up other work is read narrowly; recovery limited to reasonable, proved cost — *Niranjan Shankar Golikari* allows in-term restraint; punitive bonds disfavoured). |

### C6. DEFAULT / DELAYED-PAYMENT INTEREST
| | |
|---|---|
| **Severity** | **MED** (HIGH where the rate is extortionate or compounding). |
| **Why risky** | An extortionate or heavily-compounding default-interest rate is a **penalty** and can be reduced; conversely, silence on interest leaves your client unable to recover the time-value of withheld money. |
| **Look for** | No interest on delayed payment at all; a rate that is **penal/compounding** monthly; interest that runs from before the due date; interest stacked on top of an LD clause for the same delay (double recovery). |
| **Redline / fix** | Set a **commercially reasonable** rate (often pegged to a bank/MCLR/SBI rate + margin); simple unless compounding is justified; avoid double-dipping with LD; for the paying side, flag a penal rate for §74 reduction. |
| **India hook** | **§74** (penal interest reducible); **Interest Act 1978**; for B2B supply, the **MSMED Act 2006 §§15–16** imposes a *statutory* compound interest (3× bank rate) for delayed payment to a registered micro/small enterprise — **a buyer cannot contract this out**; check counterparty MSME status. |

### C7. GOVERNING LAW & EXCLUSIVE JURISDICTION
| | |
|---|---|
| **Severity** | **MED→HIGH** — the wrong forum clause can hand the dispute to an inconvenient/hostile court or collide with the arbitration clause. |
| **Why risky** | An exclusive-jurisdiction clause that names a court with **no nexus** to the parties or cause, or that **contradicts the arbitration clause**, breeds preliminary jurisdiction battles. |
| **Look for** | "Courts at ___ shall have exclusive jurisdiction" where neither party nor cause connects to that place; governing-law/jurisdiction mismatch; jurisdiction clause that survives despite an arbitration clause (which court — only for §9/§34/§37 supervisory matters?); foreign governing law for a purely domestic Indian deal (raises enforceability/§28 issues). |
| **Redline / fix** | Pick a forum with a **real nexus** (registered office, performance, cause of action); harmonise with arbitration (courts named **only** for arbitration-support and execution); for a domestic deal keep Indian law; spell out "subject to the arbitration clause". |
| **India hook** | **§28 Contract Act** — an absolute ouster of *all* courts is void, **but** parties may choose **one** of two or more courts that *do* have jurisdiction (*Swastik Gases v. Indian Oil*; *A.B.C. Laminart*; "alone/only/exclusive" not essential if intent is clear). Cannot confer jurisdiction on a court that has none. |

### C8. ARBITRATION CLAUSE — seat, rules, appointment
| | |
|---|---|
| **Severity** | **HIGH** — a defective arbitration clause is litigated *about* for years before the merits are ever reached. |
| **Why risky** | Confusing **seat vs venue**, no governing rules, no appointing mechanism, or a **unilateral right of one party to appoint the sole arbitrator** can render the clause unworkable or the appointment **invalid**. |
| **Look for** | "Venue" used where "**seat**" is meant (seat fixes the supervisory court and curial law — *BGS SGS Soma*); no rules (ad-hoc vs institutional unspecified); **one party alone appoints the sole arbitrator / controls the panel** — *invalid*; even-numbered tribunal; no §21 notice mechanism; clause silent on number of arbitrators/language; stamping not addressed (an unstamped arbitration agreement is now enforceable for reference per *In re Interplay* (2023, 7-judge) — but the **deficiency must still be cured**). |
| **Redline / fix** | Fix the **seat** (and separately the hearing venue), the **rules/institution**, **number** of arbitrators, language, and a **neutral appointment** mechanism; never give one side unilateral appointment power; add the §21 trigger; ensure the underlying instrument is properly **stamped**. |
| **India hook** | A&C Act 1996; **\*TRF Ltd v. Energo Engineering\*** and **\*Perkins Eastman v. HSCC\*** — a person ineligible to be an arbitrator (interested party / its nominee) **cannot appoint** the sole arbitrator; **unilateral-appointment clauses are invalid** (reaffirmed by the 5-judge bench in *Central Organisation for Railway Electrification* (2024)). *BALCO* (seat), *BGS SGS Soma* (seat = exclusive supervisory jurisdiction). |

### C9. IP ASSIGNMENT & MORAL RIGHTS
| | |
|---|---|
| **Severity** | **HIGH** in any development/creative/consulting/employment deal — without it the client never owns what it paid for. |
| **Why risky** | India has **no "work made for hire" default that vests all IP in the commissioner** for independent contractors; absent a **present, written assignment**, the **author/contractor keeps copyright**. Future-IP and moral rights need express treatment. |
| **Look for** | "Will assign" (a mere agreement to assign, not a present assignment) instead of "hereby assigns"; assignment of **future** works without the statutory specifics; no waiver/treatment of **moral rights** (paternity/integrity); contractor/employee carve-out for "background IP" undefined; no assignment of **registrations, applications, and the right to sue for past infringement**; software source code / deliverables ownership silent. |
| **Redline / fix** | Use "**hereby assigns** all present and future IP" with a **present-tense vesting** and a further-assurance clause; identify the works/medium (Copyright Act §19 needs the assignment in writing, signed, specifying the work, rights, term and territory — **an unspecified term defaults to 5 years and territory to India**, §19(5)-(6)); address **moral rights** (§57 — paternity and integrity rights are **author-personal and cannot be wholly assigned**; obtain a waiver/consent to modifications to the extent permissible); cover background IP and licence-back. |
| **India hook** | **Copyright Act §17** (first ownership — employer owns work made in the course of employment **under a contract of service**, *but* a commissioned contractor under a contract *for* service is different); **§18–19** (assignment must be written, signed, specific; §19(5)/(6) default 5-year term / India territory); **§57 moral rights** survive assignment. Patents Act for invention assignments. |

### C10. NON-COMPETE / NON-SOLICIT (in-term vs post-term)
| | |
|---|---|
| **Severity** | **HIGH** for post-employment/post-term restraints (usually **void**); MED for in-term and non-solicit. |
| **Why risky** | A **post-termination non-compete is void** in India regardless of reasonableness; relying on it gives false comfort and may even taint the clause. Non-solicit of employees/customers is on safer but not certain ground. |
| **Look for** | Post-employment / post-termination non-compete (any duration, any geography) — **void**; "reasonable" qualifiers that the drafter thinks save it (they don't, post-term); over-broad non-solicit that is really a non-compete in disguise; garden-leave used to extend a restraint; in business-sale context, a goodwill non-compete that exceeds the **§27 Exception 1** limits (reasonable, local, while a similar business is carried on by the buyer). |
| **Redline / fix** | Keep restraints to the **in-term** period (an employee may be restrained from competing **during** employment — *Niranjan Shankar Golikari*, *Superintendence Co. v. Krishan Murgai*); rely on **confidentiality + IP + non-solicit + garden leave** rather than a post-term non-compete; for an M&A goodwill sale, fit it inside §27 Exception 1; never promise the client a post-term non-compete is enforceable. |
| **India hook** | **§27 Contract Act** — every agreement in restraint of trade is **void** to that extent; **only** the sale-of-goodwill exception (Exception 1) and partnership exceptions (§§11(2), 36, 54 Partnership Act) survive. *Percept D'Mark v. Zaheer Khan*; *Gujarat Bottling v. Coca Cola* (in-term exclusivity OK). |

### C11. CONFIDENTIALITY — scope, duration, breach-as-condition
| | |
|---|---|
| **Severity** | **MED→HIGH** (HIGH where trade secrets/IP are the deal's value). |
| **Why risky** | India has **no standalone trade-secret statute** — confidentiality is protected mainly by **contract** (and equity/breach of confidence). A weak NDA is the only wall around the secret. |
| **Look for** | "Confidential Information" undefined or too narrow; **no perpetual carve-out for trade secrets** (a 2–3-year tail is fine for ordinary info, fatal for source code/formulae); standard exceptions missing (public domain, independently developed, lawfully received, **compelled by law/court**); no return/destruction-on-termination; no carve-out for regulatory/investor disclosure; breach not made a **material/condition** breach with injunctive relief acknowledged. |
| **Redline / fix** | Define CI broadly with the standard four exceptions + compelled-disclosure procedure; **perpetual** duration for trade secrets, fixed for the rest; return/destroy + certify; make breach a material breach, acknowledge **irreparable harm and the right to injunctive relief** (helps obtain interim relief — §A10 three-fold test still applies in court). |
| **India hook** | No dedicated trade-secrets Act; enforced via contract + the equitable action for breach of confidence; injunctions under the Specific Relief Act and O39 CPC. The **DPDP Act 2023** overlays personal-data confidentiality/breach-notification obligations — check if CI includes personal data. |

### C12. AUTO-RENEWAL / EVERGREEN TERM
| | |
|---|---|
| **Severity** | **MED** (HIGH in long recurring-revenue or lease contexts). |
| **Why risky** | An evergreen clause renews **automatically** unless notice is given in a tight window — the locked-in party can miss it and be bound for another full term, with a price-escalator running. |
| **Look for** | Auto-renewal with a **short, early non-renewal notice window**; renewal at **escalated price** without a cap; no maximum number of renewals; renewal that drags an out-of-date price/spec forward; lease auto-renewal pushing the **term past 11 months** (triggers compulsory registration — see C20/§C-X). |
| **Redline / fix** | Make non-renewal notice **reasonable and symmetrical**; **cap** any auto-escalation; require affirmative renewal for long terms; for leases, watch the 11-month registration line. |
| **India hook** | Lease renewal crossing 12 months → **§17 Registration Act** compulsory registration (and stamp). |

### C13. ASSIGNMENT & CHANGE-OF-CONTROL
| | |
|---|---|
| **Severity** | **MED→HIGH** — free assignment lets your counterparty substitute a weaker/hostile entity; an unconsented CoC can dump the deal on a competitor. |
| **Why risky** | If the counterparty can **assign freely** or undergo a **change of control** without consent, your client can end up bound to someone it never chose; conversely an over-tight clause can block your client's own financing/restructuring. |
| **Look for** | One-sided assignment (counterparty free, your client restricted); **CoC not a defined trigger** (acquisition of the counterparty by a competitor passes silently); assignment to affiliates without a financial-strength floor; novation vs assignment confusion (liabilities not transferred); anti-assignment that also blocks security assignment to your client's lenders. |
| **Redline / fix** | Require **prior written consent** (not unreasonably withheld) for assignment; define **change of control** and make it a consent/termination trigger; permit intra-group assignment with notice + continuing guarantee; carve out assignment to lenders by way of security. |
| **India hook** | Burden of a contract cannot be assigned without consent (novation, §62 Contract Act); benefits generally assignable unless personal or barred — confirm the clause matches. |

### C14. WARRANTY DISCLAIMER / "AS IS"
| | |
|---|---|
| **Severity** | **MED→HIGH** depending on which side. |
| **Why risky** | An "as is, no warranties express or implied" disclaimer can strip the **implied conditions of merchantability/fitness** under the Sale of Goods Act, leaving the buyer remediless; or the buyer may have given **no warranty protection** in a deal where it needed it. |
| **Look for** | Blanket "as is/with all faults"; disclaimer of **all implied terms** (Sale of Goods Act §§14–17 implied conditions); no express warranty of title, quiet possession, non-infringement, fitness for the stated purpose; disclaimer attempting to exclude liability for **fraud/concealment** (void); consumer-facing disclaimer (may be an **unfair contract term** under CP Act 2019). |
| **Redline / fix** | For the buyer, preserve **core express warranties** (title, non-infringement, conformity to spec, fitness for the communicated purpose) and resist a total implied-term exclusion; for the seller, scope the "as is" precisely and keep statutory/fraud liability un-excludable; flag CP Act 2019 unfair-term exposure in B2C. |
| **India hook** | **Sale of Goods Act 1930 §§14–17** (implied conditions/warranties — can be negatived by express agreement, §62, **except** where it would defeat statute or operate as fraud); **CP Act 2019 §2(46) "unfair contract"** (one-sided terms voidable). |

### C15. FORCE MAJEURE — gaps vs §56 frustration
| | |
|---|---|
| **Severity** | **MED→HIGH** (HIGH in supply/construction/long-term contracts). |
| **Why risky** | If a force-majeure clause **exists but is narrow/closed-list**, the party **cannot fall back on §56 frustration** for an event the clause covers but excludes — the courts hold the contract itself has provided for the contingency. A poorly drafted FM clause thus *removes* a statutory escape. |
| **Look for** | Closed FM list omitting pandemics/epidemics, government action, cyber events, supply-chain failure; **no notice + mitigation + duration cap + termination-on-prolonged-FM**; FM covering only one party; payment obligations not excluded from FM relief; "mere commercial hardship/price rise" assumed to be FM (it is **not** — *Energy Watchdog*); reliance on §56 where an FM clause already governs (won't lie). |
| **Redline / fix** | Use an **open-ended** list ("including but not limited to … and any other event beyond reasonable control"); add **notice, ongoing mitigation, a relief cap, and a long-stop termination right** if FM persists; clarify which obligations are suspended; remember **§56 is unavailable once the FM clause covers the field** — so draft the clause to do the work. |
| **India hook** | **§32 Contract Act** (FM as a contingent-contract clause) vs **§56** (frustration of an un-provided-for supervening impossibility). *Satyabrata Ghose v. Mugneeram*; **\*Energy Watchdog v. CERC\*** (where parties have an FM clause, §56 does not apply; commercial hardship ≠ frustration). |

### C16. PAYMENT / SET-OFF / SELF-HELP / SECURITY
| | |
|---|---|
| **Severity** | **MED→HIGH**. |
| **Why risky** | One-sided **set-off** (counterparty can net off disputed amounts), **self-help** remedies (repossession, switching off SaaS, drawing on a guarantee on mere allegation), and weak/over-strong **security** terms shift leverage dramatically. |
| **Look for** | Counterparty's **unilateral set-off / withholding** right with no reciprocal right; **on-demand bank guarantee** invocable on bare claim with no fraud/irretrievable-injury check; deemed-acceptance/auto-debit; security (charge/pledge/lien) created without the formalities; **no GST/TDS allocation**; payment milestones unhooked from deliverables; cross-default pulling unrelated obligations. |
| **Redline / fix** | Make set-off **mutual** and limited to **undisputed/adjudicated** sums; restrict self-help/BG invocation to admitted defaults or independent verification; perfect any security correctly; allocate **GST, TDS, and other taxes** expressly; tie payment to acceptance. |
| **India hook** | An **unconditional bank guarantee** is honoured on demand save for **established fraud or irretrievable injustice** (*U.P. Co-op. Federation v. Singh Consultants*; *Hindustan Construction*) — so an on-demand BG is a powerful self-help tool to resist or wield. GST + TDS (Income-tax Act) allocation; charge registration (Companies Act §77 for company security — 30-day filing). |

### C17. LAYERED INDEMNITY & LIABILITY CAPS (the interaction)
| | |
|---|---|
| **Severity** | **HIGH** — the **interaction** between C1 and C2 is where parties get blindsided. |
| **Why risky** | An indemnity that is **inside** the liability cap may be worthless against a large third-party claim; an indemnity **outside** the cap (plus uncapped consequential exclusion carve-outs) can blow past the deal value. Super-caps, baskets, and de-minimis thresholds interlock and are easy to mis-stitch. |
| **Look for** | Cap and indemnity drafted in different clauses that **contradict** (one says "notwithstanding anything", the other "subject to the cap"); IP/confidentiality/data-breach indemnity not given a **super-cap**; consequential-loss exclusion that accidentally guts the indemnity; **basket/de-minimis** thresholds and **survival periods** for warranty-indemnity claims missing or mismatched; "sole remedy" language colliding with the indemnity. |
| **Redline / fix** | Build a single, explicit **liability architecture**: general cap → super-cap for IP/confidentiality/data → **uncapped** for fraud/wilful/death-PI/statutory; state expressly whether each indemnity sits inside, at the super-cap, or outside; align baskets, de-minimis, and survival; remove contradictory "notwithstanding"/"subject to" cross-references. |
| **India hook** | Same as C1/C2 — §§124-125, §73/§74; courts give effect to a **clearly drafted** allocation but read an **ambiguous exclusion contra proferentem** against the party relying on it. |

### C18. UNILATERAL AMENDMENT / WAIVER / ENTIRE-AGREEMENT
| | |
|---|---|
| **Severity** | **MED** (HIGH where one side can change pricing/terms at will). |
| **Why risky** | A **unilateral amendment** right (common in platform/SaaS/financial T&Cs) lets the strong party rewrite the deal; an over-broad **entire-agreement** clause can wipe out relied-upon side letters/representations; a **no-waiver** clause cuts both ways. |
| **Look for** | "Provider may amend these terms at any time by posting/notice"; entire-agreement clause that **excludes pre-contractual representations** (and thus a misrepresentation remedy) without preserving fraud; no-oral-modification with no exception for written variation; **waiver by conduct** silently barred where your client relied on indulgence; severability missing (one bad clause risks the rest). |
| **Redline / fix** | Require **mutual written amendment** (or, if unilateral notice survives commercially, give the other side a **terminate-on-change** right); preserve **fraud and the agreed side letters** in the entire-agreement clause; keep severability; make no-waiver reciprocal. |
| **India hook** | A contract is varied by mutual consent (**§62 Contract Act**); entire-agreement clauses **cannot exclude liability for fraud/misrepresentation** (§17–18, §19); standard-form unilateral terms in unequal bargaining can be unconscionable (*Brojo Nath*; CP Act 2019 unfair-term). |

### C19. ONE-SIDED REPS & WARRANTIES + DISCLOSURE SCHEDULE
| | |
|---|---|
| **Severity** | **MED→HIGH** (HIGH in M&A/share-purchase/asset deals). |
| **Why risky** | Reps & warranties allocate **unknown risk**; if one side gives extensive warranties and the other none, or there is **no disclosure schedule / knowledge qualifier / survival period / materiality threshold**, the warrantor carries open-ended exposure (or the buyer gets no protection). |
| **Look for** | Asymmetric warranty package; **no disclosure schedule** (so any known issue becomes a breach); no **knowledge ("to the best of … knowledge after due enquiry")** qualifiers; no **survival/limitation period** for warranty claims, or one longer than statute; no **materiality/de-minimis/basket**; title/authority/no-conflict/litigation/tax/compliance/IP warranties missing; no **anti-sandbagging / pro-sandbagging** position taken. |
| **Redline / fix** | Balance the package; attach a **disclosure schedule** that qualifies the warranties; add **knowledge and materiality** qualifiers; set a **survival period** and a **financial cap/basket/de-minimis**; for the buyer, secure key warranties (title, authority, tax, litigation, IP, compliance) hard; decide the sandbagging position expressly. |
| **India hook** | Misrepresentation/fraud remedies (**§§17–19 Contract Act** — rescission + damages); a disclosed fact cannot ground a misrep claim; tax/stamp warranties matter because **unstamped/under-stamped deal documents are inadmissible** (see §C-X). |

### C20. FULL-AND-FINAL RELEASE OF UNKNOWN CLAIMS
| | |
|---|---|
| **Severity** | **MED→HIGH** — a release signed in settlement/exit can extinguish claims your client doesn't yet know it has. |
| **Why risky** | A broad "**full and final settlement of all claims, known or unknown, present or future**" release bars later claims — including ones the releasing party would never have given up knowingly. Conversely a narrow release leaves the paying party exposed to a second bite. |
| **Look for** | "All claims whatsoever, known or unknown" with **no carve-outs**; release of **fraud / future statutory dues / accrued-but-unknown** claims; employee final-settlement releases waiving **statutory dues (gratuity/PF/bonus)** that **cannot** be contracted away; mutual vs one-way release mismatch; no carve-out for **subsisting indemnities/confidentiality/IP** that should survive the release. |
| **Redline / fix** | For the releasing party, **carve out** fraud, accrued statutory entitlements, and known live disputes, and prefer "known claims as of the date"; for the paying party, make it **mutual and as broad as enforceable**; preserve surviving obligations (indemnity/confidentiality/IP) expressly; in employment, never purport to release **non-waivable statutory dues**. |
| **India hook** | A release/accord-and-satisfaction is binding **unless vitiated by fraud, coercion, undue influence, or signed under economic duress / "no dues" extracted as a precondition of release of admitted dues** (*Union of India v. Master Construction*; *National Insurance v. Boghara Polyfab* — a discharge voucher obtained by coercion does not bar the claim, relevant to arbitration too). **Statutory dues (Payment of Gratuity Act §4, EPF, Bonus Act) are non-waivable.** |

### §C-X — INDIA CROSS-CUTTING (applies to EVERY instrument above)

These two heads are **not clause-specific** — sweep them across the whole document on every transactional review/draft, and flag as **HIGH** because they go to **admissibility and validity**, not just allocation of risk.

| | |
|---|---|
| **STAMP DUTY & REGISTRATION** | **HIGH.** An **unstamped or under-stamped instrument is inadmissible in evidence** (Indian Stamp Act §35 / State Stamp Acts) — it cannot be relied on in court or arbitration until duty + penalty are paid; the arbitration agreement *within* it is now severable for reference (*In re Interplay*, 2023) **but the stamping defect must still be cured before the award/decree**. **Compulsorily registrable** instruments that are unregistered **pass no title / are inoperative**: a **sale of immovable property, a gift, and a lease exceeding 11 months/one year** must be registered (**§17 Registration Act 1908**; §49 — unregistered = inadmissible to affect immovable property, save the limited collateral-purpose proviso). **Look for:** stamp value left blank / paid on the wrong State's rate / lease drafted at 11 months to dodge registration but auto-renewing past it (C12); a deed relied on as title that was never registered; e-stamping particulars missing. **Fix:** state the correct **stamp duty and registration** obligation, who bears it, and the deadline; register where §17 requires; never let a client rely on an unstamped/unregistered instrument as proof. |
| **E-SIGNATURE & ELECTRONIC EXECUTION** | **MED→HIGH.** An electronically-executed contract is valid, **but** the method must qualify. **Look for:** "may be signed electronically / by DocuSign" with no compliance hook; documents that **cannot** be electronically executed (negotiable instruments, powers of attorney, trusts, wills, and any sale/conveyance of immovable property — **First Schedule, IT Act**); aggregator/clickwrap acceptance not tied to a §10A-valid method; cross-border e-execution where stamping/registration still demands physical/e-stamp. **Fix:** confirm the e-sign method is a **valid electronic/digital signature** and that the instrument is **not** in the IT-Act exclusion list; for excluded documents (and registrable deeds), require wet-ink + registration. **India hook:** **§10A IT Act 2000** (validity of contracts formed electronically); §3/§3A and the Second Schedule (electronic vs digital signature); **First Schedule exclusions**. |

---

# §D — HOW DONNA SHOULD USE THIS

0. **This rubric DRIVES the redlining/review.** Every draft and every uploaded document is reviewed *against this rubric* — work the §A cross-cutting list, the matching §B type checklist, and (for contracts/deeds) the §C clause map, in that order. Tag **every** issue **HIGH / MED / LOW** and name **which side** it helps and which it wounds. If an issue is **not covered by this rubric**, you **may** fall back on general legal training knowledge — **but you must tell the user you are going beyond the rubric**, using this exact phrasing:

   > ⚠️ Beyond the rubric — general principle: …

   Never present a beyond-the-rubric point as if it came from the rubric, and still tag it HIGH/MED/LOW + side.

1. **Tag every issue HIGH / MED / LOW** and say **which side it helps or hurts** ("This limitation gap is a sword for the defendant — if you act for the plaintiff, plead the saving and a condonation fallback now").
2. **Ask which party the user represents** if it isn't stated, *before* redlining — the same defect is offence for one side and defence for the other. Ask once, then proceed.
3. **Never fabricate citations, sections, dates, names, or amounts.** Unknown facts stay as `________` placeholders. Cite case law only via the kanoon_search → kanoon_verify_case flow; if a section is cited under the wrong code for its offence date, flag it.
4. **Flag statutory bars proactively** — limitation, jurisdiction, S.80/S.12A/S.138/S.69 pre-conditions, non-joinder — **even if the user didn't ask** and even if it hurts their case. Surfacing a bar before filing is the whole value; a missed bar surfaces in court as a dismissal.
5. **On an upload/review**, run §A as a checklist, then the matching §B type checklist; output a short triage table (Issue | Severity | Side | Fix) followed by the redlined clauses. **On drafting**, build the document right the first time and call out the HIGH risks you resolved and any the user must decide.
6. **Don't over-warn.** Lead with HIGH filing-killers; group MED/LOW; never let polish bury a limitation bar.
