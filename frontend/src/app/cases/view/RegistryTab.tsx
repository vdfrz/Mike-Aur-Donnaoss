"use client";

import { useState, useEffect, useRef } from "react";
import { Copy, Trash2, ChevronUp, ChevronDown, Plus, Sparkles, ExternalLink, Check } from "lucide-react";
import type { CaseDocument } from "@/app/components/shared/types";
import {
    listCaseParties,
    createCaseParty,
    updateCaseParty,
    deleteCaseParty,
    reorderCaseParties,
    aiPopulateParties,
    listCaseAnnexures,
    createCaseAnnexure,
    updateCaseAnnexure,
    deleteCaseAnnexure,
    reorderCaseAnnexures,
    aiPopulateAnnexures,
    listCaseCitations,
    deleteCaseCitation,
    generateCasesReferred,
    generateAuthorities,
    type CasePartyRecord,
    type CaseAnnexure,
    type CaseCitation,
} from "@/app/lib/mikeApi";

interface RegistryTabProps {
    caseId: string;
    documents: CaseDocument[];
    /** True once the case has analysis findings — gates the one-time
     *  auto-populate of parties from the analysis. */
    hasFindings?: boolean;
}

export function RegistryTab({ caseId, documents, hasFindings = false }: RegistryTabProps) {
    const [parties, setParties] = useState<CasePartyRecord[]>([]);
    const [annexures, setAnnexures] = useState<CaseAnnexure[]>([]);
    const [citations, setCitations] = useState<{ judgments: CaseCitation[]; statutes: CaseCitation[] }>({ judgments: [], statutes: [] });
    const [loading, setLoading] = useState(true);
    const [numberingChanged, setNumberingChanged] = useState(false);
    const [copiedSlug, setCopiedSlug] = useState<string | null>(null);

    // Party form state
    const [showAddParty, setShowAddParty] = useState(false);
    const [newPartyName, setNewPartyName] = useState("");
    const [newPartySide, setNewPartySide] = useState<"petitioner" | "respondent">("petitioner");
    const [addPartyLoading, setAddPartyLoading] = useState(false);

    // Edit states
    const [editingPartyId, setEditingPartyId] = useState<string | null>(null);
    const [editingPartyName, setEditingPartyName] = useState("");
    const [editingPartyRole, setEditingPartyRole] = useState("");

    const [editingAnnexureId, setEditingAnnexureId] = useState<string | null>(null);
    const [editingAnnexureDesc, setEditingAnnexureDesc] = useState("");
    const [editingAnnexureDate, setEditingAnnexureDate] = useState("");

    // Loading states for actions
    const [aiPopulatingParties, setAiPopulatingParties] = useState(false);
    const [aiPopulatingAnnexures, setAiPopulatingAnnexures] = useState(false);
    const [generatingCasesReferred, setGeneratingCasesReferred] = useState(false);
    const [generatingAuthorities, setGeneratingAuthorities] = useState(false);
    const [successMessage, setSuccessMessage] = useState<string | null>(null);

    const loadData = async () => {
        try {
            setLoading(true);
            const [partiesRes, annexuresRes, citationsRes] = await Promise.all([
                listCaseParties(caseId),
                listCaseAnnexures(caseId),
                listCaseCitations(caseId),
            ]);
            setParties(partiesRes.parties);
            setAnnexures(annexuresRes.annexures);
            setCitations(citationsRes);
            setNumberingChanged(false);
        } catch (err) {
            console.error("[registry] load error:", err);
        } finally {
            setLoading(false);
        }
    };

    useEffect(() => {
        loadData();
    }, [caseId]);

    // Auto-populate parties from the analysis the first time the registry is
    // opened for an analysed case that has none yet. Runs once; the user can
    // still edit, reorder or delete afterwards.
    const autoPopulatedRef = useRef(false);
    useEffect(() => {
        if (loading || autoPopulatedRef.current) return;
        if (hasFindings && parties.length === 0) {
            autoPopulatedRef.current = true;
            handleAiPopulateParties();
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [loading, hasFindings, parties.length]);

    const copyToClipboard = (slug: string) => {
        navigator.clipboard.writeText(`@${slug}`);
        setCopiedSlug(slug);
        setTimeout(() => setCopiedSlug(null), 2000);
    };

    const handleAddParty = async () => {
        if (!newPartyName.trim()) return;
        try {
            setAddPartyLoading(true);
            const newParty = await createCaseParty(caseId, {
                name: newPartyName.trim(),
                side: newPartySide,
            });
            setParties([...parties, newParty]);
            setNewPartyName("");
            setShowAddParty(false);
        } catch (err) {
            console.error("[registry] add party error:", err);
        } finally {
            setAddPartyLoading(false);
        }
    };

    const handleUpdateParty = async (partyId: string) => {
        try {
            const result = await updateCaseParty(caseId, partyId, {
                name: editingPartyName || undefined,
                role_label: editingPartyRole || undefined,
            });
            if (result.party) {
                setParties(parties.map((p) => (p.id === partyId ? result.party! : p)));
            }
            setEditingPartyId(null);
        } catch (err) {
            console.error("[registry] update party error:", err);
        }
    };

    const handleDeleteParty = async (partyId: string) => {
        try {
            await deleteCaseParty(caseId, partyId);
            setParties(parties.filter((p) => p.id !== partyId));
        } catch (err) {
            console.error("[registry] delete party error:", err);
        }
    };

    const handleReorderParties = async (side: "petitioner" | "respondent", direction: "up" | "down", index: number) => {
        const sideParties = parties.filter((p) => p.side === side).sort((a, b) => a.serial_no - b.serial_no);
        if ((direction === "up" && index === 0) || (direction === "down" && index === sideParties.length - 1)) return;

        const newIndex = direction === "up" ? index - 1 : index + 1;
        const newOrder = [...sideParties];
        [newOrder[index], newOrder[newIndex]] = [newOrder[newIndex], newOrder[index]];

        try {
            await reorderCaseParties(caseId, side, newOrder.map((p) => p.id));
            setParties(
                parties.map((p) => ({
                    ...p,
                    serial_no: newOrder.findIndex((np) => np.id === p.id) + 1,
                }))
            );
            setNumberingChanged(true);
        } catch (err) {
            console.error("[registry] reorder parties error:", err);
        }
    };

    const handleAiPopulateParties = async () => {
        try {
            setAiPopulatingParties(true);
            const result = await aiPopulateParties(caseId);
            setParties(result.parties);
            setNumberingChanged(true);
            setSuccessMessage(`Added ${result.seeded} parties`);
            setTimeout(() => setSuccessMessage(null), 3000);
        } catch (err) {
            console.error("[registry] ai populate parties error:", err);
        } finally {
            setAiPopulatingParties(false);
        }
    };

    const handleUpdateAnnexure = async (annexureId: string) => {
        try {
            const result = await updateCaseAnnexure(caseId, annexureId, {
                description: editingAnnexureDesc || undefined,
                doc_date: editingAnnexureDate || undefined,
            });
            if (result.annexure) {
                setAnnexures(annexures.map((a) => (a.id === annexureId ? result.annexure! : a)));
            }
            setEditingAnnexureId(null);
        } catch (err) {
            console.error("[registry] update annexure error:", err);
        }
    };

    const handleDeleteAnnexure = async (annexureId: string) => {
        try {
            await deleteCaseAnnexure(caseId, annexureId);
            setAnnexures(annexures.filter((a) => a.id !== annexureId));
        } catch (err) {
            console.error("[registry] delete annexure error:", err);
        }
    };

    const handleReorderAnnexures = async (side: "P" | "R" | "C", direction: "up" | "down", index: number) => {
        const sideAnnexures = annexures.filter((a) => a.side === side).sort((a, b) => a.serial_no - b.serial_no);
        if ((direction === "up" && index === 0) || (direction === "down" && index === sideAnnexures.length - 1)) return;

        const newIndex = direction === "up" ? index - 1 : index + 1;
        const newOrder = [...sideAnnexures];
        [newOrder[index], newOrder[newIndex]] = [newOrder[newIndex], newOrder[index]];

        try {
            await reorderCaseAnnexures(caseId, side, newOrder.map((a) => a.id));
            setAnnexures(
                annexures.map((a) => ({
                    ...a,
                    serial_no: newOrder.findIndex((na) => na.id === a.id) + 1,
                }))
            );
            setNumberingChanged(true);
        } catch (err) {
            console.error("[registry] reorder annexures error:", err);
        }
    };

    const handleAiPopulateAnnexures = async () => {
        try {
            setAiPopulatingAnnexures(true);
            const result = await aiPopulateAnnexures(caseId);
            setAnnexures(result.annexures);
            setNumberingChanged(true);
            setSuccessMessage(`Added ${result.seeded} annexures`);
            setTimeout(() => setSuccessMessage(null), 3000);
        } catch (err) {
            console.error("[registry] ai populate annexures error:", err);
        } finally {
            setAiPopulatingAnnexures(false);
        }
    };

    const handleGenerateCasesReferred = async () => {
        try {
            setGeneratingCasesReferred(true);
            await generateCasesReferred(caseId);
            setSuccessMessage("List of Cases Referred generated");
            setTimeout(() => setSuccessMessage(null), 3000);
        } catch (err) {
            console.error("[registry] generate cases referred error:", err);
        } finally {
            setGeneratingCasesReferred(false);
        }
    };

    const handleGenerateAuthorities = async () => {
        try {
            setGeneratingAuthorities(true);
            await generateAuthorities(caseId);
            setSuccessMessage("List of Authorities generated");
            setTimeout(() => setSuccessMessage(null), 3000);
        } catch (err) {
            console.error("[registry] generate authorities error:", err);
        } finally {
            setGeneratingAuthorities(false);
        }
    };

    const handleToggleAnnexure = async (documentId: string) => {
        const existing = annexures.find((a) => a.document_id === documentId);
        if (existing) {
            await handleDeleteAnnexure(existing.id);
        } else {
            try {
                const newAnnexure = await createCaseAnnexure(caseId, { document_id: documentId });
                setAnnexures([...annexures, newAnnexure]);
                setNumberingChanged(true);
            } catch (err) {
                console.error("[registry] create annexure error:", err);
            }
        }
    };

    const handleDeleteCitation = async (citationId: string) => {
        try {
            await deleteCaseCitation(caseId, citationId);
            setCitations((prev) => ({
                judgments: prev.judgments.filter((j) => j.id !== citationId),
                statutes: prev.statutes.filter((s) => s.id !== citationId),
            }));
        } catch (err) {
            console.error("[registry] delete citation error:", err);
        }
    };

    if (loading) {
        return (
            <div className="flex h-full items-center justify-center">
                <div className="h-6 w-6 animate-spin rounded-full border-2 border-gray-300 border-t-gray-700" />
            </div>
        );
    }

    const petitioners = parties.filter((p) => p.side === "petitioner").sort((a, b) => a.serial_no - b.serial_no);
    const respondents = parties.filter((p) => p.side === "respondent").sort((a, b) => a.serial_no - b.serial_no);

    return (
        <div className="p-6 space-y-6 overflow-y-auto h-full">
            {successMessage && (
                <div className="rounded-lg bg-emerald-50 border border-emerald-200 px-4 py-3">
                    <p className="text-sm text-emerald-800 flex items-center gap-2">
                        <Check className="h-4 w-4" />
                        {successMessage}
                    </p>
                </div>
            )}

            {numberingChanged && (
                <div className="rounded-lg bg-amber-50 border border-amber-200 px-4 py-3">
                    <p className="text-xs text-amber-800">
                        Numbering updated. Regenerate outputs / rebuild drafts to apply.
                    </p>
                </div>
            )}

            {/* PARTIES SECTION */}
            <div className="rounded-lg border border-gray-200 bg-white">
                <div className="border-b border-gray-200 px-5 py-3 flex items-center justify-between">
                    <h2 className="text-sm font-medium text-gray-900">Parties</h2>
                    <div className="flex items-center gap-2">
                        <button
                            onClick={handleAiPopulateParties}
                            disabled={aiPopulatingParties}
                            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs font-medium bg-blue-50 text-blue-700 border border-blue-200 hover:bg-blue-100 disabled:opacity-50 transition-colors"
                        >
                            <Sparkles className="h-3.5 w-3.5" />
                            {aiPopulatingParties ? "Populating…" : "AI Populate"}
                        </button>
                        <button
                            onClick={() => setShowAddParty(true)}
                            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs font-medium bg-blue-50 text-blue-700 border border-blue-200 hover:bg-blue-100 transition-colors"
                        >
                            <Plus className="h-3.5 w-3.5" />
                            Add party
                        </button>
                    </div>
                </div>

                <div className="px-5 py-4 space-y-4">
                    {/* Petitioners */}
                    {petitioners.length > 0 && (
                        <div>
                            <p className="text-[11px] font-medium text-gray-500 uppercase tracking-wide mb-2">Petitioners</p>
                            <div className="space-y-2">
                                {petitioners.map((party, idx) => (
                                    <div
                                        key={party.id}
                                        className="flex items-center gap-2.5 p-3 rounded-md bg-gray-50 border border-gray-100"
                                    >
                                        <span className="inline-flex items-center justify-center w-5 h-5 rounded-full bg-blue-100 text-blue-700 text-[10px] font-semibold shrink-0">
                                            {party.serial_no}
                                        </span>

                                        {editingPartyId === party.id ? (
                                            <input
                                                autoFocus
                                                value={editingPartyName}
                                                onChange={(e) => setEditingPartyName(e.target.value)}
                                                className="flex-1 min-w-0 text-sm text-gray-900 border border-gray-200 rounded px-2 py-1 focus:outline-none focus:ring-1 focus:ring-blue-500"
                                            />
                                        ) : (
                                            <button
                                                onClick={() => {
                                                    setEditingPartyId(party.id);
                                                    setEditingPartyName(party.name);
                                                    setEditingPartyRole(party.role_label || "");
                                                }}
                                                className="flex-1 min-w-0 text-left text-sm text-gray-900 hover:text-blue-600"
                                            >
                                                {party.name}
                                            </button>
                                        )}

                                        <span className="font-mono text-xs bg-gray-100 text-gray-700 px-2 py-1 rounded border border-gray-200 flex items-center gap-1.5 shrink-0">
                                            @{party.slug}
                                            <button
                                                onClick={() => copyToClipboard(party.slug)}
                                                className="p-0.5 hover:bg-gray-200 rounded transition-colors"
                                            >
                                                {copiedSlug === party.slug ? (
                                                    <Check className="h-3 w-3 text-green-600" />
                                                ) : (
                                                    <Copy className="h-3 w-3 text-gray-400" />
                                                )}
                                            </button>
                                        </span>

                                        {editingPartyId === party.id ? (
                                            <input
                                                value={editingPartyRole}
                                                onChange={(e) => setEditingPartyRole(e.target.value)}
                                                placeholder="default"
                                                className="w-24 text-xs text-gray-700 border border-gray-200 rounded px-2 py-1 focus:outline-none focus:ring-1 focus:ring-blue-500"
                                            />
                                        ) : (
                                            <span className="text-xs text-gray-500 w-24">{party.role_label || "default"}</span>
                                        )}

                                        {editingPartyId === party.id && (
                                            <button
                                                onClick={() => handleUpdateParty(party.id)}
                                                className="px-2 py-1 rounded text-xs font-medium bg-blue-500 text-white hover:bg-blue-600"
                                            >
                                                Save
                                            </button>
                                        )}

                                        <div className="flex items-center gap-1 shrink-0">
                                            <button
                                                onClick={() => handleReorderParties("petitioner", "up", idx)}
                                                disabled={idx === 0}
                                                className="p-1 hover:bg-gray-200 rounded disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
                                            >
                                                <ChevronUp className="h-3.5 w-3.5 text-gray-400" />
                                            </button>
                                            <button
                                                onClick={() => handleReorderParties("petitioner", "down", idx)}
                                                disabled={idx === petitioners.length - 1}
                                                className="p-1 hover:bg-gray-200 rounded disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
                                            >
                                                <ChevronDown className="h-3.5 w-3.5 text-gray-400" />
                                            </button>
                                        </div>

                                        <button
                                            onClick={() => handleDeleteParty(party.id)}
                                            className="p-1 hover:bg-red-50 rounded text-gray-400 hover:text-red-500 transition-colors"
                                        >
                                            <Trash2 className="h-3.5 w-3.5" />
                                        </button>
                                    </div>
                                ))}
                            </div>
                        </div>
                    )}

                    {/* Respondents */}
                    {respondents.length > 0 && (
                        <div>
                            <p className="text-[11px] font-medium text-gray-500 uppercase tracking-wide mb-2">Respondents</p>
                            <div className="space-y-2">
                                {respondents.map((party, idx) => (
                                    <div
                                        key={party.id}
                                        className="flex items-center gap-2.5 p-3 rounded-md bg-gray-50 border border-gray-100"
                                    >
                                        <span className="inline-flex items-center justify-center w-5 h-5 rounded-full bg-red-100 text-red-700 text-[10px] font-semibold shrink-0">
                                            {party.serial_no}
                                        </span>

                                        {editingPartyId === party.id ? (
                                            <input
                                                autoFocus
                                                value={editingPartyName}
                                                onChange={(e) => setEditingPartyName(e.target.value)}
                                                className="flex-1 min-w-0 text-sm text-gray-900 border border-gray-200 rounded px-2 py-1 focus:outline-none focus:ring-1 focus:ring-blue-500"
                                            />
                                        ) : (
                                            <button
                                                onClick={() => {
                                                    setEditingPartyId(party.id);
                                                    setEditingPartyName(party.name);
                                                    setEditingPartyRole(party.role_label || "");
                                                }}
                                                className="flex-1 min-w-0 text-left text-sm text-gray-900 hover:text-blue-600"
                                            >
                                                {party.name}
                                            </button>
                                        )}

                                        <span className="font-mono text-xs bg-gray-100 text-gray-700 px-2 py-1 rounded border border-gray-200 flex items-center gap-1.5 shrink-0">
                                            @{party.slug}
                                            <button
                                                onClick={() => copyToClipboard(party.slug)}
                                                className="p-0.5 hover:bg-gray-200 rounded transition-colors"
                                            >
                                                {copiedSlug === party.slug ? (
                                                    <Check className="h-3 w-3 text-green-600" />
                                                ) : (
                                                    <Copy className="h-3 w-3 text-gray-400" />
                                                )}
                                            </button>
                                        </span>

                                        {editingPartyId === party.id ? (
                                            <input
                                                value={editingPartyRole}
                                                onChange={(e) => setEditingPartyRole(e.target.value)}
                                                placeholder="default"
                                                className="w-24 text-xs text-gray-700 border border-gray-200 rounded px-2 py-1 focus:outline-none focus:ring-1 focus:ring-blue-500"
                                            />
                                        ) : (
                                            <span className="text-xs text-gray-500 w-24">{party.role_label || "default"}</span>
                                        )}

                                        {editingPartyId === party.id && (
                                            <button
                                                onClick={() => handleUpdateParty(party.id)}
                                                className="px-2 py-1 rounded text-xs font-medium bg-blue-500 text-white hover:bg-blue-600"
                                            >
                                                Save
                                            </button>
                                        )}

                                        <div className="flex items-center gap-1 shrink-0">
                                            <button
                                                onClick={() => handleReorderParties("respondent", "up", idx)}
                                                disabled={idx === 0}
                                                className="p-1 hover:bg-gray-200 rounded disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
                                            >
                                                <ChevronUp className="h-3.5 w-3.5 text-gray-400" />
                                            </button>
                                            <button
                                                onClick={() => handleReorderParties("respondent", "down", idx)}
                                                disabled={idx === respondents.length - 1}
                                                className="p-1 hover:bg-gray-200 rounded disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
                                            >
                                                <ChevronDown className="h-3.5 w-3.5 text-gray-400" />
                                            </button>
                                        </div>

                                        <button
                                            onClick={() => handleDeleteParty(party.id)}
                                            className="p-1 hover:bg-red-50 rounded text-gray-400 hover:text-red-500 transition-colors"
                                        >
                                            <Trash2 className="h-3.5 w-3.5" />
                                        </button>
                                    </div>
                                ))}
                            </div>
                        </div>
                    )}

                    {parties.length === 0 && (
                        <p className="text-xs text-gray-400 py-4 text-center">
                            No parties yet. Add one or click AI populate.
                        </p>
                    )}

                    {/* Add party form */}
                    {showAddParty && (
                        <div className="border-t border-gray-100 pt-4 mt-4">
                            <div className="flex items-center gap-2.5">
                                <input
                                    autoFocus
                                    value={newPartyName}
                                    onChange={(e) => setNewPartyName(e.target.value)}
                                    placeholder="Party name"
                                    className="flex-1 text-sm text-gray-900 border border-gray-200 rounded px-3 py-2 focus:outline-none focus:ring-1 focus:ring-blue-500"
                                    onKeyDown={(e) => {
                                        if (e.key === "Enter") handleAddParty();
                                        if (e.key === "Escape") setShowAddParty(false);
                                    }}
                                />
                                <select
                                    value={newPartySide}
                                    onChange={(e) => setNewPartySide(e.target.value as "petitioner" | "respondent")}
                                    className="text-xs text-gray-700 border border-gray-200 rounded px-2 py-2 focus:outline-none focus:ring-1 focus:ring-blue-500"
                                >
                                    <option value="petitioner">Petitioner</option>
                                    <option value="respondent">Respondent</option>
                                </select>
                                <button
                                    onClick={handleAddParty}
                                    disabled={addPartyLoading}
                                    className="px-3 py-2 rounded text-xs font-medium bg-blue-500 text-white hover:bg-blue-600 disabled:opacity-50"
                                >
                                    Add
                                </button>
                                <button
                                    onClick={() => setShowAddParty(false)}
                                    className="px-3 py-2 rounded text-xs font-medium bg-gray-100 text-gray-700 hover:bg-gray-200"
                                >
                                    Cancel
                                </button>
                            </div>
                        </div>
                    )}
                </div>
            </div>

            {/* ANNEXURES SECTION */}
            <div className="rounded-lg border border-gray-200 bg-white">
                <div className="border-b border-gray-200 px-5 py-3 flex items-center justify-between">
                    <h2 className="text-sm font-medium text-gray-900">Annexures</h2>
                    <button
                        onClick={handleAiPopulateAnnexures}
                        disabled={aiPopulatingAnnexures}
                        className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs font-medium bg-blue-50 text-blue-700 border border-blue-200 hover:bg-blue-100 disabled:opacity-50 transition-colors"
                    >
                        <Sparkles className="h-3.5 w-3.5" />
                        {aiPopulatingAnnexures ? "Populating…" : "AI Populate"}
                    </button>
                </div>

                <div className="px-5 py-4 space-y-3">
                    {documents.map((doc) => {
                        const annex = annexures.find((a) => a.document_id === doc.document_id);
                        const isDesignated = !!annex;

                        if (!isDesignated) {
                            return (
                                <div key={doc.document_id} className="flex items-center gap-3 p-3 rounded-md bg-gray-50 border border-gray-100">
                                    <input
                                        type="checkbox"
                                        checked={false}
                                        onChange={() => handleToggleAnnexure(doc.document_id)}
                                        className="rounded border-gray-300 text-blue-500 focus:ring-blue-500"
                                    />
                                    <span className="text-xs text-gray-700">{doc.filename || doc.document_id.slice(0, 8)}</span>
                                </div>
                            );
                        }

                        return (
                            <div key={annex.id} className="flex items-center gap-2.5 p-3 rounded-md bg-gray-50 border border-gray-100">
                                <input
                                    type="checkbox"
                                    checked={true}
                                    onChange={() => handleToggleAnnexure(doc.document_id)}
                                    className="rounded border-gray-300 text-blue-500 focus:ring-blue-500"
                                />

                                <span className="inline-flex items-center justify-center w-5 h-5 rounded-full bg-amber-100 text-amber-700 text-[10px] font-semibold shrink-0">
                                    {annex.side}-{annex.serial_no}
                                </span>

                                <span className="font-mono text-xs bg-gray-100 text-gray-700 px-2 py-1 rounded border border-gray-200 shrink-0">
                                    #{annex.slug}
                                </span>

                                {editingAnnexureId === annex.id ? (
                                    <>
                                        <input
                                            autoFocus
                                            value={editingAnnexureDesc}
                                            onChange={(e) => setEditingAnnexureDesc(e.target.value)}
                                            placeholder="Description"
                                            className="flex-1 min-w-0 text-xs text-gray-900 border border-gray-200 rounded px-2 py-1 focus:outline-none focus:ring-1 focus:ring-blue-500"
                                        />
                                        <input
                                            type="date"
                                            value={editingAnnexureDate}
                                            onChange={(e) => setEditingAnnexureDate(e.target.value)}
                                            className="text-xs text-gray-700 border border-gray-200 rounded px-2 py-1 focus:outline-none focus:ring-1 focus:ring-blue-500"
                                        />
                                        <button
                                            onClick={() => handleUpdateAnnexure(annex.id)}
                                            className="px-2 py-1 rounded text-xs font-medium bg-blue-500 text-white hover:bg-blue-600"
                                        >
                                            Save
                                        </button>
                                    </>
                                ) : (
                                    <>
                                        <button
                                            onClick={() => {
                                                setEditingAnnexureId(annex.id);
                                                setEditingAnnexureDesc(annex.description || "");
                                                setEditingAnnexureDate(annex.doc_date || "");
                                            }}
                                            className="flex-1 min-w-0 text-left text-xs text-gray-600 hover:text-blue-600"
                                        >
                                            {annex.description || "—"}
                                        </button>
                                        <span className="text-xs text-gray-500">{annex.doc_date || "—"}</span>
                                    </>
                                )}

                                <div className="flex items-center gap-1 shrink-0">
                                    <button
                                        onClick={() => {
                                            const sideAnnex = annexures.filter((a) => a.side === annex.side).sort((a, b) => a.serial_no - b.serial_no);
                                            const idx = sideAnnex.findIndex((a) => a.id === annex.id);
                                            if (idx > 0) handleReorderAnnexures(annex.side, "up", idx);
                                        }}
                                        className="p-1 hover:bg-gray-200 rounded transition-colors"
                                    >
                                        <ChevronUp className="h-3.5 w-3.5 text-gray-400" />
                                    </button>
                                    <button
                                        onClick={() => {
                                            const sideAnnex = annexures.filter((a) => a.side === annex.side).sort((a, b) => a.serial_no - b.serial_no);
                                            const idx = sideAnnex.findIndex((a) => a.id === annex.id);
                                            if (idx < sideAnnex.length - 1) handleReorderAnnexures(annex.side, "down", idx);
                                        }}
                                        className="p-1 hover:bg-gray-200 rounded transition-colors"
                                    >
                                        <ChevronDown className="h-3.5 w-3.5 text-gray-400" />
                                    </button>
                                </div>

                                <button
                                    onClick={() => handleDeleteAnnexure(annex.id)}
                                    className="p-1 hover:bg-red-50 rounded text-gray-400 hover:text-red-500 transition-colors"
                                >
                                    <Trash2 className="h-3.5 w-3.5" />
                                </button>
                            </div>
                        );
                    })}

                    {documents.length === 0 && (
                        <p className="text-xs text-gray-400 py-4 text-center">No documents to designate</p>
                    )}
                </div>
            </div>

            {/* CITATIONS SECTION */}
            <div className="rounded-lg border border-gray-200 bg-white">
                <div className="border-b border-gray-200 px-5 py-3 flex items-center justify-between">
                    <h2 className="text-sm font-medium text-gray-900">Citations</h2>
                    <div className="flex items-center gap-2">
                        <button
                            onClick={handleGenerateCasesReferred}
                            disabled={generatingCasesReferred}
                            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs font-medium bg-blue-50 text-blue-700 border border-blue-200 hover:bg-blue-100 disabled:opacity-50 transition-colors"
                        >
                            {generatingCasesReferred ? "Generating…" : "Cases Referred"}
                        </button>
                        <button
                            onClick={handleGenerateAuthorities}
                            disabled={generatingAuthorities}
                            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs font-medium bg-blue-50 text-blue-700 border border-blue-200 hover:bg-blue-100 disabled:opacity-50 transition-colors"
                        >
                            {generatingAuthorities ? "Generating…" : "Authorities"}
                        </button>
                    </div>
                </div>

                <div className="px-5 py-4 space-y-4">
                    {/* Judgments */}
                    {citations.judgments.length > 0 && (
                        <div>
                            <p className="text-[11px] font-medium text-gray-500 uppercase tracking-wide mb-2">Judgments</p>
                            <div className="space-y-2">
                                {citations.judgments.map((cit) => (
                                    <div key={cit.id} className="flex items-center gap-2.5 p-3 rounded-md bg-gray-50 border border-gray-100">
                                        <span className={`inline-flex items-center justify-center px-2 py-1 rounded-full text-[10px] font-semibold shrink-0 ${
                                            cit.status === "cited"
                                                ? "bg-emerald-100 text-emerald-700"
                                                : "bg-gray-100 text-gray-600"
                                        }`}>
                                            {cit.status}
                                        </span>

                                        {cit.kanoon_url ? (
                                            <a
                                                href={cit.kanoon_url}
                                                target="_blank"
                                                rel="noopener noreferrer"
                                                className="flex-1 min-w-0 text-xs text-blue-600 hover:text-blue-700 hover:underline"
                                            >
                                                {cit.title || `Case ${cit.id.slice(0, 8)}`}
                                            </a>
                                        ) : (
                                            <span className="flex-1 min-w-0 text-xs text-gray-700">
                                                {cit.title || `Case ${cit.id.slice(0, 8)}`}
                                            </span>
                                        )}

                                        {cit.pdf_document_id && (
                                            <span className="inline-flex items-center px-2 py-0.5 rounded-full bg-amber-100 text-amber-700 text-[10px] font-medium shrink-0">
                                                PDF
                                            </span>
                                        )}

                                        <button
                                            onClick={() => handleDeleteCitation(cit.id)}
                                            className="p-1 hover:bg-red-50 rounded text-gray-400 hover:text-red-500 transition-colors shrink-0"
                                        >
                                            <Trash2 className="h-3.5 w-3.5" />
                                        </button>
                                    </div>
                                ))}
                            </div>
                        </div>
                    )}

                    {/* Statutes */}
                    {citations.statutes.length > 0 && (
                        <div>
                            <p className="text-[11px] font-medium text-gray-500 uppercase tracking-wide mb-2">Statutes</p>
                            <div className="space-y-2">
                                {citations.statutes.map((cit) => (
                                    <div key={cit.id} className="flex items-center gap-2.5 p-3 rounded-md bg-gray-50 border border-gray-100">
                                        <span className={`inline-flex items-center justify-center px-2 py-1 rounded-full text-[10px] font-semibold shrink-0 ${
                                            cit.status === "cited"
                                                ? "bg-emerald-100 text-emerald-700"
                                                : "bg-gray-100 text-gray-600"
                                        }`}>
                                            {cit.status}
                                        </span>

                                        <span className="flex-1 min-w-0 text-xs text-gray-700 font-medium">
                                            {cit.statute}
                                            {cit.section_number && ` § ${cit.section_number}`}
                                        </span>

                                        <button
                                            onClick={() => handleDeleteCitation(cit.id)}
                                            className="p-1 hover:bg-red-50 rounded text-gray-400 hover:text-red-500 transition-colors shrink-0"
                                        >
                                            <Trash2 className="h-3.5 w-3.5" />
                                        </button>
                                    </div>
                                ))}
                            </div>
                        </div>
                    )}

                    {citations.judgments.length === 0 && citations.statutes.length === 0 && (
                        <p className="text-xs text-gray-400 py-4 text-center">No citations yet</p>
                    )}
                </div>
            </div>
        </div>
    );
}
