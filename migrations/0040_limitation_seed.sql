-- Seed the Limitation Act, 1963 into the statute database so a "Limitation
-- Check" workflow has real data to reason over. Schema defined in
-- 0037_statute_db.sql; the FTS5 triggers auto-sync statute_sections_fts.
-- Idempotent via INSERT OR IGNORE.
--
-- NOTE: statute id 9000 is a fixed high id chosen to avoid colliding with the
-- small auto-assigned ids used by data/seed_statutes.sql (IPC/BNS/CrPC/... = 1-6).
-- Section ids are 9001+ for the same reason.
--
-- v1 disclaimer: the periods and starting points below are the well-known
-- provisions of the Limitation Act, 1963, stated in plain words. The user
-- should verify against the bare Act / current amendments before relying on
-- them in a filing.

-- 1. Statute record -----------------------------------------------------------
INSERT OR IGNORE INTO statutes (id, short_name, full_title, year, status, replaced_by, category, language)
VALUES (9000, 'Limitation Act', 'The Limitation Act, 1963', 1963, 'active', NULL, 'procedural', 'en');

-- 2. Core sections + key Schedule Articles -------------------------------------
INSERT OR IGNORE INTO statute_sections (id, statute_id, section_number, title, body) VALUES
(9001, 9000, 'Section 3',
 'Bar of limitation',
 'Every suit instituted, appeal preferred, or application made after the prescribed period shall be dismissed, although limitation has not been set up as a defence. The period is computed from the date the cause of action accrues as fixed by the Schedule.'),

(9002, 9000, 'Section 5',
 'Extension of prescribed period in certain cases (condonation of delay)',
 'An appeal or application (other than a suit) may be admitted after the prescribed period if the appellant or applicant satisfies the court that he had sufficient cause for not preferring it within time. Time begins to run when the period under the Schedule expires; the delay thereafter must be explained.'),

(9003, 9000, 'Article 1',
 'Suit for the balance due on a mutual, open and current account',
 'Period: 3 years. Time begins to run from the close of the year in which the last item admitted or proved is entered in the account, such year being computed as in the account.'),

(9004, 9000, 'Article 14',
 'Suit for the price of goods sold and delivered where no fixed period of credit is agreed',
 'Period: 3 years. Time begins to run from the date of delivery of the goods.'),

(9005, 9000, 'Article 18',
 'Suit for the price of work done by the plaintiff where no time has been fixed for payment',
 'Period: 3 years. Time begins to run from the date when the work is done.'),

(9006, 9000, 'Article 19',
 'Suit for money payable for money lent',
 'Period: 3 years. Time begins to run from the date when the loan is made.'),

(9007, 9000, 'Article 54',
 'Suit for specific performance of a contract',
 'Period: 3 years. Time begins to run from the date fixed for the performance, or, if no such date is fixed, when the plaintiff has notice that performance is refused.'),

(9008, 9000, 'Article 55',
 'Suit for compensation for the breach of any contract, express or implied, not specially provided for',
 'Period: 3 years. Time begins to run when the contract is broken, or (where there are successive breaches) when the breach in respect of which the suit is instituted occurs, or (where the breach is continuing) when it ceases.'),

(9009, 9000, 'Article 58',
 'Suit to obtain any other declaration',
 'Period: 3 years. Time begins to run when the right to sue first accrues.'),

(9010, 9000, 'Article 59',
 'Suit to cancel or set aside an instrument or decree, or for the rescission of a contract',
 'Period: 3 years. Time begins to run when the facts entitling the plaintiff to have the instrument or decree cancelled or set aside, or the contract rescinded, first become known to him.'),

(9011, 9000, 'Article 65',
 'Suit for possession of immovable property or any interest therein based on title',
 'Period: 12 years. Time begins to run when the possession of the defendant becomes adverse to the plaintiff.'),

(9012, 9000, 'Article 113',
 'Any suit for which no period of limitation is provided elsewhere in the Schedule',
 'Period: 3 years (residuary article for suits). Time begins to run when the right to sue accrues.'),

(9013, 9000, 'Article 116',
 'Appeal under the Code of Civil Procedure, 1908, to a High Court',
 'Period: 90 days. Time begins to run from the date of the decree or order appealed from.'),

(9014, 9000, 'Article 117',
 'Appeal under the Code of Civil Procedure, 1908, to any court other than a High Court',
 'Period: 30 days. Time begins to run from the date of the decree or order appealed from.'),

(9015, 9000, 'Article 136',
 'Application for the execution of any decree (other than a decree granting a mandatory injunction) or order of a civil court',
 'Period: 12 years. Time begins to run when the decree or order becomes enforceable, or (where it directs payment of money or delivery of property to be made at a certain date or in instalments) when default in making the payment or delivery occurs.'),

(9016, 9000, 'Article 137',
 'Any application for which no period of limitation is provided elsewhere in this Division',
 'Period: 3 years (residuary article for applications). Time begins to run when the right to apply accrues.');
