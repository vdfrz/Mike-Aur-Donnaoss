#!/usr/bin/env python3
"""Test mike model with 8 detailed prompts. Save outputs for review."""
import subprocess
import os
import re

PROMPTS = {
    "01_affidavit": """Draft an affidavit to be filed in the Delhi High Court.
Deponent: Mr. Vikram Singh, son of Late Sh. Harjit Singh, aged 45 years, residing at C-204, Sector 15, Rohini, Delhi-110089.
Purpose: To declare that he has never been convicted of any criminal offence and is not facing any pending criminal proceedings.
Date of affidavit: 15th October 2024.
Verification at New Delhi.
To be filed in support of his application for police verification.""",

    "02_section_91": """Draft an application under Section 91 of the CrPC.
Court: Court of Sh. Rajesh Kumar, Metropolitan Magistrate, Saket Court, New Delhi.
Case Number: CC No. 2341/2023.
Filed by: Mr. Anil Verma s/o Sh. Suresh Verma, complainant.
Against: Mr. Mohan Lal s/o Sh. Ram Lal, accused.
Case offences: Sections 420 and 406 IPC for cheating of Rs. 15,00,000.
Prayer: Direct the accused to produce his bank statements from HDFC Bank Account No. 50100234567890 for the period 1st January 2022 to 31st December 2023.
Date: 20th October 2024.""",

    "03_reply_notice": """Draft a reply to a legal notice.
Client: Mrs. Sunita Kapoor, residing at Flat 401, Sapphire Heights, Andheri West, Mumbai-400053.
Notice received from: Adv. Pranav Mehta on behalf of M/s Acme Builders Pvt Ltd.
Notice dated: 5th September 2024.
Notice demanded: Payment of Rs. 3,50,000 as alleged outstanding delayed possession penalty.
Our reply: Deny all allegations. Possession was already taken on 12th March 2024 as per receipt dated 12.03.2024. No penalty is due per Clause 8.4 of the Builder-Buyer Agreement dated 10th February 2020.
Demand withdrawal of notice within 15 days failing which legal action will be initiated.""",

    "04_writ_petition": """Draft a writ petition under Article 226 of the Constitution.
Court: Bombay High Court.
Petitioner: Mr. Rohan Desai, age 38, residing at 12 Marine Drive, Mumbai-400020, occupation: Chartered Accountant.
Respondent No. 1: Income Tax Department through Commissioner of Income Tax, Mumbai.
Respondent No. 2: Assessing Officer Ward 12(3)(2), Mumbai.
Grievance: The petitioner's bank account at SBI Marine Drive Branch was attached on 18th September 2024 under Section 226(3) of the Income Tax Act for alleged tax dues of Rs. 22,50,000 for AY 2021-22, without serving the demand notice required under Section 156.
Prayer: Quash the attachment order dated 18.09.2024 and direct refund of attached amount.""",

    "05_power_of_attorney": """Draft a General Power of Attorney.
Principal/Donor: Mr. Arjun Reddy, son of Mr. Krishna Reddy, age 55, residing at 23 Banjara Hills Road No. 12, Hyderabad-500034.
Attorney/Agent: His brother Mr. Karthik Reddy, son of Mr. Krishna Reddy, age 50, residing at 28 Jubilee Hills, Hyderabad-500033.
Powers granted: To manage and operate Principal's bank accounts at HDFC Bank Banjara Hills Branch and ICICI Bank Jubilee Hills Branch, to collect rent from properties at Plot 47 Madhapur and Plot 89 Gachibowli, to file and respond to tax returns.
Duration: 2 years from 1st November 2024.
Executed at Hyderabad on 28th October 2024.""",

    "06_criminal_complaint": """Draft a criminal complaint under Section 200 CrPC.
Court: Court of Chief Judicial Magistrate, Pune.
Complainant: Mr. Sanjay Joshi, son of Mr. Vinay Joshi, age 42, Proprietor of M/s Joshi Trading Co., residing at 8 Karve Road, Pune-411004.
Accused: Mr. Deepak Mishra, son of Mr. Mahesh Mishra, age 38, residing at 45 FC Road, Pune-411005.
Offences: Sections 420 (cheating), 406 (criminal breach of trust) and 506 (criminal intimidation) of IPC.
Facts: On 5th June 2024, accused promised to supply 500 quintals of basmati rice at Rs. 80 per kg. Complainant paid Rs. 20,00,000 as advance via RTGS UTR No. SBIN0123456789 dated 8th June 2024 to HDFC Bank Account No. 50100876543210 of the accused. Delivery date promised: 15th July 2024. No delivery made. On 20th September 2024 accused threatened the complainant to withdraw the matter.""",

    "07_settlement": """Draft a mutual settlement agreement.
Party A: Mr. Rakesh Kumar, son of Sh. Mohan Kumar, residing at H.No. 234, Sector 8, Faridabad-121006, Haryana.
Party B: Ms. Pooja Sharma, daughter of Sh. Anil Sharma, residing at Flat 7B, Green Park Apartments, Gurgaon-122001, Haryana.
Subject: Settlement of matrimonial disputes between parties married on 12th December 2015 at Faridabad, living separately since 1st January 2023.
Terms: (i) Mutual consent divorce to be filed at Family Court Faridabad. (ii) Party A to pay Rs. 25,00,000 as one-time alimony via demand draft within 30 days. (iii) Custody of minor daughter Aisha (age 6) to remain with Party B with visitation rights to Party A every 2nd weekend. (iv) Both parties withdraw all pending cases. (v) Each party to bear their own legal costs.
Date of agreement: 25th October 2024.""",

    "08_written_statement": """Draft a written statement in defense.
Court: Court of Civil Judge (Senior Division), Bangalore.
Suit No: O.S. No. 567/2024.
Plaintiff: M/s Sunrise Textiles Ltd through its Director Mr. Manish Bhatia.
Defendant: M/s Bangalore Garment Exporters, Proprietor Mr. Ravi Kumar, age 47, residing at 28 6th Main Road, Indiranagar, Bangalore-560038.
Suit for: Recovery of Rs. 18,75,000 with interest at 12% p.a. for alleged unpaid invoices dated 5th April 2024 and 20th May 2024.
Defense: (i) Deny all allegations. (ii) Plaintiff's invoices already paid via cheque no. 567890 dated 10th June 2024 of Karnataka Bank Indiranagar Branch. (iii) Plaintiff acknowledged receipt of payment via WhatsApp message dated 11.06.2024. (iv) Suit is barred by waiver and estoppel. (v) Counter-claim of Rs. 5,00,000 for damages due to defective goods supplied.""",
}

ANSI_RE = re.compile(r'\x1b(\[[0-9;]*[a-zA-Z]|\][^\x07]*\x07|\[\?[0-9]+[hl]|\[2026[hl]|\[\?25[hl])')

def clean(text):
    return ANSI_RE.sub('', text)

OUTPUT_DIR = "/tmp/mike_demo"
os.makedirs(OUTPUT_DIR, exist_ok=True)

for name, prompt in PROMPTS.items():
    path = f"{OUTPUT_DIR}/{name}.txt"
    if os.path.exists(path) and os.path.getsize(path) > 500:
        print(f"Skipping {name} (already done)")
        continue
    print(f"\n{'='*80}\nRunning: {name}\n{'='*80}")
    try:
        result = subprocess.run(
            ["ollama", "run", "mike"],
            input=prompt,
            capture_output=True,
            text=True,
            timeout=600,
        )
        output = clean(result.stdout)
    except subprocess.TimeoutExpired as e:
        output = f"[TIMED OUT after 600s. Partial: {clean(e.stdout.decode() if e.stdout else '')[:5000]}]"
    with open(path, "w") as f:
        f.write(f"PROMPT:\n{prompt}\n\n{'='*80}\nOUTPUT:\n{output}")
    print(f"Saved -> {path} ({len(output)} chars)")

print(f"\nAll outputs in {OUTPUT_DIR}/")
