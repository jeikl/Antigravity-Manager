import React, { useEffect, useState, useRef, useMemo } from 'react';
import { listen } from '@tauri-apps/api/event';
import ModalDialog from '../common/ModalDialog';
import { useTranslation } from 'react-i18next';
import { request as invoke } from '../../utils/request';
import { Trash2, Search, X, Copy, CheckCircle, ChevronLeft, ChevronRight, ChevronUp, ChevronDown, RefreshCw, User, Sparkles, FileCode2, Eye, EyeOff } from 'lucide-react';

import { AppConfig } from '../../types/config';
import { formatCompactNumber } from '../../utils/format';
import { useAccountStore } from '../../stores/useAccountStore';
import { isTauri } from '../../utils/env';
import { copyToClipboard } from '../../utils/clipboard';


interface ProxyRequestLog {
    id: string;
    timestamp: number;
    method: string;
    url: string;
    status: number;
    duration: number;
    model?: string;
    mapped_model?: string;
    error?: string;
    request_body?: string;
    upstream_request_body?: string;
    response_body?: string;
    request_headers?: string;
    upstream_request_headers?: string;
    response_headers?: string;
    input_tokens?: number;
    output_tokens?: number;
    cached_tokens?: number;
    account_email?: string;
    protocol?: string;  // "openai" | "anthropic" | "gemini"
}

interface ProxyStats {
    total_requests: number;
    success_count: number;
    error_count: number;
}

interface ProxyMonitorProps {
    className?: string;
}

// Log Table Component
interface LogTableProps {
    logs: ProxyRequestLog[];
    loading: boolean;
    onLogClick: (log: ProxyRequestLog) => void;
    t: any;
}

const LogTable: React.FC<LogTableProps> = ({
    logs,
    loading,
    onLogClick,
    t
}) => {
    return (
        <div
            className="flex-1 overflow-y-auto overflow-x-auto bg-white dark:bg-base-100"
        >
            <table className="table table-xs w-full">
                <thead className="bg-gray-50 dark:bg-base-200 text-gray-500 sticky top-0 z-10">
                    <tr>
                        <th style={{ width: '60px' }}>{t('monitor.table.status')}</th>
                        <th style={{ width: '60px' }}>{t('monitor.table.method')}</th>
                        <th style={{ width: '220px' }}>{t('monitor.table.model')}</th>
                        <th style={{ width: '70px' }}>{t('monitor.table.protocol')}</th>
                        <th style={{ width: '140px' }}>{t('monitor.table.account')}</th>
                        <th style={{ width: '180px' }}>{t('monitor.table.path')}</th>
                        <th className="text-right" style={{ width: '90px' }}>{t('monitor.table.usage')}</th>
                        <th className="text-right" style={{ width: '80px' }}>{t('monitor.table.duration')}</th>
                        <th className="text-right" style={{ width: '80px' }}>{t('monitor.table.time')}</th>
                    </tr>
                </thead>
                <tbody className="font-mono text-gray-700 dark:text-gray-300">
                    {logs.map((log) => (
                        <tr
                            key={log.id}
                            className="hover:bg-blue-50 dark:hover:bg-blue-900/20 cursor-pointer"
                            onClick={() => onLogClick(log)}
                        >
                            <td style={{ width: '60px' }}>
                                <span className={`badge badge-xs text-white border-none ${log.status >= 200 && log.status < 400 ? 'badge-success' : 'badge-error'}`}>
                                    {log.status}
                                </span>
                            </td>
                            <td className="font-bold" style={{ width: '60px' }}>{log.method}</td>
                            <td className="text-blue-600 truncate" style={{ width: '220px', maxWidth: '220px' }}>
                                {log.mapped_model && log.model !== log.mapped_model
                                    ? `${log.model} => ${log.mapped_model}`
                                    : (log.model || '-')}
                            </td>
                            <td style={{ width: '70px' }}>
                                {log.protocol && (
                                    <span className={`badge badge-xs text-white border-none ${log.protocol === 'openai' ? 'bg-green-500' :
                                        log.protocol === 'anthropic' ? 'bg-orange-500' :
                                            log.protocol === 'gemini' ? 'bg-blue-500' : 'bg-gray-400'
                                        }`}>
                                        {log.protocol === 'openai' ? 'OpenAI' :
                                            log.protocol === 'anthropic' ? 'Claude' :
                                                log.protocol === 'gemini' ? 'Gemini' : log.protocol}
                                    </span>
                                )}
                            </td>
                            <td className="text-gray-600 dark:text-gray-400 truncate text-[10px]" style={{ width: '140px', maxWidth: '140px' }} title={log.account_email || ''}>
                                {log.account_email ? log.account_email.replace(/(.{3}).*(@.*)/, '$1***$2') : '-'}
                            </td>
                            <td className="truncate" style={{ width: '180px', maxWidth: '180px' }}>{log.url}</td>
                            <td className="text-right text-[9px]" style={{ width: '90px' }}>
                                {log.input_tokens != null && (() => {
                                    const totalIn = (log.cached_tokens && log.cached_tokens > log.input_tokens)
                                        ? log.input_tokens + log.cached_tokens
                                        : log.input_tokens;
                                    return (
                                        <div>
                                            <div>{t('monitor.input')}: {formatCompactNumber(totalIn)}</div>
                                            {log.cached_tokens ? (
                                                <div className="text-emerald-600 dark:text-emerald-400 font-medium text-[8.5px]">
                                                    ({t('monitor.cached', '缓')}: {formatCompactNumber(log.cached_tokens)})
                                                </div>
                                            ) : null}
                                        </div>
                                    );
                                })()}
                                {log.output_tokens != null && <div>{t('monitor.output')}: {formatCompactNumber(log.output_tokens)}</div>}
                            </td>
                            <td className="text-right" style={{ width: '80px' }}>{log.duration}ms</td>
                            <td className="text-right text-[10px]" style={{ width: '80px' }}>
                                {new Date(log.timestamp).toLocaleTimeString()}
                            </td>
                        </tr>
                    ))}
                </tbody>
            </table>

            {/* Loading indicator */}
            {loading && (
                <div className="flex items-center justify-center p-4 bg-white dark:bg-base-100">
                    <div className="loading loading-spinner loading-md"></div>
                    <span className="ml-3 text-sm text-gray-500">{t('common.loading')}</span>
                </div>
            )}

            {/* Empty state */}
            {!loading && logs.length === 0 && (
                <div className="flex items-center justify-center p-8 text-gray-400">
                    {t('monitor.table.empty') || '暂无请求记录'}
                </div>
            )}
        </div>
    );
};


// ==========================================
// 简要模式智能提取与映射算法
// ==========================================
function extractConcisePayload(
    rawStr: string | undefined,
    kind: 'request' | 'upstream' | 'response',
    log?: ProxyRequestLog | null
): string {
    if (!rawStr) return '';
    let obj: any;
    try {
        obj = JSON.parse(rawStr);
    } catch {
        return rawStr;
    }
    if (!obj || typeof obj !== 'object') {
        return rawStr;
    }

    // 工具声明 (完整保留 Schema，方便开发者查看工具拼接与入参定义)
    const simplifyTools = (tools: any): any => {
        if (!Array.isArray(tools)) return undefined;
        return tools;
    };

    // 简化工具调用 (保留 name, id, arguments / args)
    const simplifyToolCalls = (toolCalls: any): any => {
        if (!Array.isArray(toolCalls)) return undefined;
        return toolCalls.map((tc: any) => {
            if (!tc || typeof tc !== 'object') return tc;
            const res: any = {};
            if (tc.id) res.id = tc.id;
            if (tc.type) res.type = tc.type;
            if (tc.function && typeof tc.function === 'object') {
                res.function = {
                    name: tc.function.name,
                    arguments: tc.function.arguments !== undefined ? tc.function.arguments : {}
                };
            } else {
                if (tc.name) res.name = tc.name;
                if (tc.input !== undefined) res.input = tc.input;
                if (tc.args !== undefined) res.args = tc.args;
            }
            return res;
        });
    };

    // 简化消息内容 (Claude / OpenAI parts)
    const simplifyContent = (content: any): any => {
        if (typeof content === 'string') return content;
        if (Array.isArray(content)) {
            return content.map((item: any) => {
                if (typeof item === 'string') return item;
                if (!item || typeof item !== 'object') return item;
                // Claude tool_use 块
                if (item.type === 'tool_use') {
                    return {
                        type: 'tool_use',
                        id: item.id,
                        name: item.name,
                        input: item.input !== undefined ? item.input : {}
                    };
                }
                // Claude tool_result 块
                if (item.type === 'tool_result') {
                    return {
                        type: 'tool_result',
                        tool_use_id: item.tool_use_id,
                        ...(item.content !== undefined ? { content: item.content } : {}),
                        ...(item.is_error !== undefined ? { is_error: item.is_error } : {})
                    };
                }
                // Claude thinking 块与签名
                if (item.type === 'thinking') {
                    return {
                        type: 'thinking',
                        thinking: item.thinking,
                        ...(item.signature !== undefined ? { signature: item.signature } : {}),
                        ...(item.thought_signature !== undefined ? { thought_signature: item.thought_signature } : {}),
                        ...(item.thoughtSignature !== undefined ? { thoughtSignature: item.thoughtSignature } : {}),
                        ...(item.thinking_signature !== undefined ? { thinking_signature: item.thinking_signature } : {})
                    };
                }
                // Claude redacted_thinking 块
                if (item.type === 'redacted_thinking') {
                    return {
                        type: 'redacted_thinking',
                        data: item.data
                    };
                }
                // 文本块
                if (item.type === 'text') {
                    return item;
                }
                return item;
            });
        }
        return content;
    };

    // 简化消息列表
    const simplifyMessages = (messages: any): any => {
        if (!Array.isArray(messages)) return undefined;
        return messages.map((m: any) => {
            if (!m || typeof m !== 'object') return m;
            const res: any = { role: m.role };
            if (m.content !== undefined) {
                res.content = simplifyContent(m.content);
            }
            if (m.reasoning_content !== undefined) {
                res.reasoning_content = m.reasoning_content;
            }
            if (m.thinking !== undefined) {
                res.thinking = m.thinking;
            }
            if (m.signature !== undefined) {
                res.signature = m.signature;
            }
            if (m.thought_signature !== undefined) {
                res.thought_signature = m.thought_signature;
            }
            if (m.thinking_signature !== undefined) {
                res.thinking_signature = m.thinking_signature;
            }
            if (m.tool_calls) {
                res.tool_calls = simplifyToolCalls(m.tool_calls);
            }
            if (m.tool_call_id) {
                res.tool_call_id = m.tool_call_id;
            }
            if (m.name) {
                res.name = m.name;
            }
            return res;
        });
    };

    // 简化 Gemini 轮次 (contents)
    const simplifyGeminiContents = (contents: any): any => {
        if (!Array.isArray(contents)) return undefined;
        return contents.map((c: any) => {
            if (!c || typeof c !== 'object') return c;
            const res: any = { role: c.role };
            if (Array.isArray(c.parts)) {
                res.parts = c.parts.map((p: any) => {
                    if (!p || typeof p !== 'object') return p;

                    // 1. 优先识别工具调用 (functionCall) 并保留其名称、ID、参数与携带的加密思考签名
                    if (p.functionCall) {
                        const fcPart: any = {
                            functionCall: {
                                name: p.functionCall.name,
                                ...(p.functionCall.id ? { id: p.functionCall.id } : {}),
                                args: p.functionCall.args !== undefined ? p.functionCall.args : {}
                            }
                        };
                        if (p.thought !== undefined) fcPart.thought = p.thought;
                        if (p.thoughtSignature !== undefined) fcPart.thoughtSignature = p.thoughtSignature;
                        if (p.thought_signature !== undefined) fcPart.thought_signature = p.thought_signature;
                        if (p.signature !== undefined) fcPart.signature = p.signature;
                        return fcPart;
                    }

                    // 2. 优先识别工具响应 (functionResponse) 并保留其名称、ID、返回值与携带的签名
                    if (p.functionResponse) {
                        const frPart: any = {
                            functionResponse: {
                                name: p.functionResponse.name,
                                ...(p.functionResponse.id ? { id: p.functionResponse.id } : {}),
                                response: p.functionResponse.response !== undefined ? p.functionResponse.response : {}
                            }
                        };
                        if (p.thought !== undefined) frPart.thought = p.thought;
                        if (p.thoughtSignature !== undefined) frPart.thoughtSignature = p.thoughtSignature;
                        if (p.thought_signature !== undefined) frPart.thought_signature = p.thought_signature;
                        if (p.signature !== undefined) frPart.signature = p.signature;
                        return frPart;
                    }

                    // 3. 独立思考块 (纯思考过程，不带工具调用)
                    if (p.thought !== undefined || p.thought_signature !== undefined || p.thoughtSignature !== undefined || p.signature !== undefined) {
                        const tPart: any = {};
                        if (p.thought !== undefined) tPart.thought = p.thought;
                        if (p.thought_signature !== undefined) tPart.thought_signature = p.thought_signature;
                        if (p.thoughtSignature !== undefined) tPart.thoughtSignature = p.thoughtSignature;
                        if (p.signature !== undefined) tPart.signature = p.signature;
                        if (p.text !== undefined) tPart.text = p.text;
                        return tPart;
                    }

                    // 4. 普通文本块
                    if (p.text !== undefined) {
                        return { text: p.text };
                    }

                    return p;
                });
            }
            return res;
        });
    };

    // 简化系统提示词 (Gemini / Anthropic)
    const simplifySystemInstruction = (sys: any): any => {
        if (!sys || typeof sys !== 'object') return sys;
        if (Array.isArray(sys.parts)) {
            return {
                parts: sys.parts.map((p: any) => {
                    if (typeof p === 'string') return { text: p };
                    if (p && typeof p === 'object' && p.text !== undefined) return { text: p.text };
                    return p;
                })
            };
        }
        return sys;
    };

    // 提取用量与缓存命中率
    const simplifyUsage = (usage: any): any => {
        if (!usage || typeof usage !== 'object') return undefined;
        const res: any = {};
        const rawInput = usage.prompt_tokens ?? usage.input_tokens ?? usage.promptTokenCount;
        const output = usage.completion_tokens ?? usage.output_tokens ?? usage.candidatesTokenCount;

        let cached = usage.cached_tokens ?? usage.cache_read_input_tokens ?? usage.cachedContentTokenCount;
        if (cached == null && usage.prompt_tokens_details?.cached_tokens != null) {
            cached = usage.prompt_tokens_details.cached_tokens;
        }
        if (cached == null && usage.input_tokens_details?.cached_tokens != null) {
            cached = usage.input_tokens_details.cached_tokens;
        }

        // 计算全量上下文输入 Token (Total Context Input)
        // 1. Anthropic 官方协议: input_tokens 仅代表未缓存增量，总上下文 = input_tokens + cache_read_input_tokens
        // 2. 兼容历史日志: 若 cached > rawInput，说明 rawInput 存的是未缓存差值，做自愈加和
        let totalInput = rawInput != null ? Number(rawInput) : undefined;
        if (cached != null && totalInput != null && cached > totalInput) {
            totalInput = totalInput + Number(cached);
        } else if (usage.cache_read_input_tokens != null && usage.prompt_tokens == null && usage.promptTokenCount == null) {
            totalInput = Number(usage.input_tokens || 0) + Number(cached || 0);
        }

        const total = usage.total_tokens ?? usage.totalTokenCount ?? (totalInput != null && output != null ? totalInput + Number(output) : undefined);

        if (totalInput != null) res.input_tokens = totalInput;
        if (output != null) res.output_tokens = Number(output);
        if (total != null) res.total_tokens = Number(total);
        if (cached != null) {
            res.cached_tokens = Number(cached);
            if (totalInput != null && totalInput > 0) {
                const rate = Math.min(100, Math.max(0, (Number(cached) / totalInput) * 100));
                res.cache_hit_rate = `${rate.toFixed(1)}%`;
            }
        }
        if (usage.cache_creation_input_tokens != null) {
            res.cache_creation_input_tokens = usage.cache_creation_input_tokens;
        }
        if (usage.completion_tokens_details?.reasoning_tokens != null) {
            res.reasoning_tokens = usage.completion_tokens_details.reasoning_tokens;
        }
        if (usage.output_tokens_details?.reasoning_tokens != null) {
            res.reasoning_tokens = usage.output_tokens_details.reasoning_tokens;
        }
        return res;
    };

    const concise: any = {};

    // 保留用于标识思考块/会话的单行标识 (支持 requestId, sessionId, trace_id 等)
    const candidateSessionId =
        obj.requestId ||
        obj.request?.sessionId ||
        obj._session_id ||
        obj.session_id ||
        (log?.id ? log.id : undefined);

    if (candidateSessionId) {
        concise._session_thinking_id = candidateSessionId;
    }

    // 模型
    if (obj.model) concise.model = obj.model;

    // 思考模型配置 (开启、预算、effort、summary)
    if (obj.thinking !== undefined) concise.thinking = obj.thinking;
    if (obj.reasoning_effort !== undefined) concise.reasoning_effort = obj.reasoning_effort;
    if (obj.reasoning !== undefined) concise.reasoning = obj.reasoning;
    if (obj.summary !== undefined) concise.summary = obj.summary;
    if (obj.generationConfig?.thinkingConfig !== undefined) {
        concise.thinkingConfig = obj.generationConfig.thinkingConfig;
    } else if (obj.thinkingConfig !== undefined) {
        concise.thinkingConfig = obj.thinkingConfig;
    }

    // 系统提示词
    if (obj.system !== undefined) concise.system = obj.system;
    if (obj.systemInstruction !== undefined) concise.systemInstruction = simplifySystemInstruction(obj.systemInstruction);

    // 对话主体 (OpenAI / Claude)
    if (obj.messages) {
        concise.messages = simplifyMessages(obj.messages);
    }

    // 对话主体 (Gemini)
    if (obj.contents) {
        concise.contents = simplifyGeminiContents(obj.contents);
    }

    // 工具声明
    if (obj.tools) {
        concise.tools = simplifyTools(obj.tools);
    }

    // Antigravity 专用的 request 嵌套包装层 (核心：正确映射原中转报文的嵌套层级)
    if (obj.request && typeof obj.request === 'object') {
        const innerReq: any = {};

        // 单行会话标识
        if (obj.request.sessionId) {
            innerReq.sessionId = obj.request.sessionId;
        }

        // 思考配置 (thinkingConfig / generationConfig)
        if (obj.request.generationConfig?.thinkingConfig !== undefined) {
            innerReq.thinkingConfig = obj.request.generationConfig.thinkingConfig;
        } else if (obj.request.thinkingConfig !== undefined) {
            innerReq.thinkingConfig = obj.request.thinkingConfig;
        }

        // 系统提示词
        if (obj.request.systemInstruction !== undefined) {
            innerReq.systemInstruction = simplifySystemInstruction(obj.request.systemInstruction);
        }

        // 对话主体与思考块 (Gemini contents 或 Claude messages)
        if (obj.request.contents) {
            innerReq.contents = simplifyGeminiContents(obj.request.contents);
        }
        if (obj.request.messages) {
            innerReq.messages = simplifyMessages(obj.request.messages);
        }

        // 工具声明
        if (obj.request.tools) {
            innerReq.tools = simplifyTools(obj.request.tools);
        }

        concise.request = innerReq;
    }

    // 响应：思考块与思考签名 (顶层响应或非流式)
    if (obj.thinking !== undefined) concise.thinking = obj.thinking;
    if (obj.thinking_signature !== undefined) concise.thinking_signature = obj.thinking_signature;
    if (obj.thought_signature !== undefined) concise.thought_signature = obj.thought_signature;
    if (obj.signature !== undefined) concise.signature = obj.signature;

    // 响应：Choices / Candidates / 聚合响应
    if (obj.choices && Array.isArray(obj.choices)) {
        concise.choices = obj.choices.map((c: any) => {
            const choiceRes: any = { index: c.index };
            if (c.finish_reason) choiceRes.finish_reason = c.finish_reason;
            if (c.message) {
                choiceRes.message = {
                    role: c.message.role,
                    ...(c.message.reasoning_content !== undefined ? { reasoning_content: c.message.reasoning_content } : {}),
                    ...(c.message.thinking !== undefined ? { thinking: c.message.thinking } : {}),
                    ...(c.message.thinking_signature !== undefined ? { thinking_signature: c.message.thinking_signature } : {}),
                    ...(c.message.thought_signature !== undefined ? { thought_signature: c.message.thought_signature } : {}),
                    ...(c.message.signature !== undefined ? { signature: c.message.signature } : {}),
                    ...(c.message.content !== undefined ? { content: c.message.content } : {}),
                    ...(c.message.tool_calls ? { tool_calls: simplifyToolCalls(c.message.tool_calls) } : {})
                };
            } else if (c.delta) {
                choiceRes.delta = {
                    role: c.delta.role,
                    ...(c.delta.reasoning_content !== undefined ? { reasoning_content: c.delta.reasoning_content } : {}),
                    ...(c.delta.thinking !== undefined ? { thinking: c.delta.thinking } : {}),
                    ...(c.delta.thinking_signature !== undefined ? { thinking_signature: c.delta.thinking_signature } : {}),
                    ...(c.delta.thought_signature !== undefined ? { thought_signature: c.delta.thought_signature } : {}),
                    ...(c.delta.signature !== undefined ? { signature: c.delta.signature } : {}),
                    ...(c.delta.content !== undefined ? { content: c.delta.content } : {}),
                    ...(c.delta.tool_calls ? { tool_calls: simplifyToolCalls(c.delta.tool_calls) } : {})
                };
            }
            return choiceRes;
        });
    }

    if (obj.candidates && Array.isArray(obj.candidates)) {
        concise.candidates = obj.candidates.map((cand: any) => {
            const candRes: any = {};
            if (cand.finishReason) candRes.finishReason = cand.finishReason;
            if (cand.content) {
                candRes.content = simplifyGeminiContents([cand.content])?.[0] || cand.content;
            }
            return candRes;
        });
    }

    if (obj.content !== undefined && !obj.messages && !obj.choices && !obj.request) {
        concise.content = simplifyContent(obj.content);
    }
    if (obj.reasoning_content !== undefined && !obj.messages && !obj.choices) {
        concise.reasoning_content = obj.reasoning_content;
    }
    if (obj.tool_calls && !obj.messages && !obj.choices) {
        concise.tool_calls = simplifyToolCalls(obj.tool_calls);
    }

    // 用量与缓存
    const usage = simplifyUsage(obj.usage || obj.usageMetadata);
    if (usage) {
        concise.usage = usage;
    } else if (kind === 'response' && (log?.input_tokens || log?.output_tokens)) {
        const totalIn = (log.cached_tokens && log.cached_tokens > (log.input_tokens || 0))
            ? (log.input_tokens || 0) + log.cached_tokens
            : (log.input_tokens || 0);
        concise.usage = {
            input_tokens: totalIn,
            output_tokens: log.output_tokens,
            total_tokens: totalIn + (log.output_tokens || 0),
            ...(log.cached_tokens != null ? {
                cached_tokens: log.cached_tokens,
                cache_hit_rate: totalIn > 0 ? `${Math.min(100, Math.max(0, (log.cached_tokens / totalIn) * 100)).toFixed(1)}%` : undefined
            } : {})
        };
    }

    return JSON.stringify(concise, null, 2);
}

// ==========================================
// 单栏报文展示卡片（含语法高亮、独立搜索、跳转与复制）
// ==========================================
interface PayloadViewerCardProps {
    cardId: string;
    title: string;
    badge: string;
    badgeStyle: string;
    rawPayload?: string;
    concisePayload?: string;
    headersJson?: string;
    viewMode: 'concise' | 'full';
    emptyPlaceholder: string;
    onCopy: (content: string) => Promise<void>;
    isCopied: boolean;
}

const renderHighlightedJson = (
    content: string,
    searchTerm: string,
    currentMatchIndex: number,
    cardId: string
) => {
    if (!content) return null;

    // Tokenize JSON: keys, strings, booleans, null, numbers, punctuation, and whitespace
    const tokenRegex = /("(\\u[a-zA-Z0-9]{4}|\\[^u]|[^\\"])*"(\s*:)?|\b(?:true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|[{}[\],:]|[^\s"{}[\],:]+|\s+)/g;

    const trimmedSearch = searchTerm.trim();
    let searchRegex: RegExp | null = null;
    if (trimmedSearch) {
        try {
            const escaped = trimmedSearch.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
            searchRegex = new RegExp(`(${escaped})`, 'gi');
        } catch {
            searchRegex = null;
        }
    }

    let globalMatchCounter = -1;

    const renderTextWithSearch = (text: string, defaultClass: string, keyPrefix: string) => {
        if (!searchRegex) {
            return <span key={keyPrefix} className={defaultClass}>{text}</span>;
        }

        const parts = text.split(searchRegex);
        return parts.map((part, pIdx) => {
            if (!part) return null;
            if (part.toLowerCase() === trimmedSearch.toLowerCase()) {
                globalMatchCounter++;
                const isActive = globalMatchCounter === currentMatchIndex;
                return (
                    <mark
                        key={`${keyPrefix}-m-${pIdx}`}
                        id={isActive ? `active-match-${cardId}` : undefined}
                        className={`rounded-sm px-0.5 font-bold transition-all duration-150 ${
                            isActive
                                ? 'bg-amber-400 text-slate-950 ring-2 ring-amber-500 shadow-sm'
                                : 'bg-amber-500/35 text-amber-900 dark:text-amber-100'
                        }`}
                    >
                        {part}
                    </mark>
                );
            }
            return (
                <span key={`${keyPrefix}-t-${pIdx}`} className={defaultClass}>
                    {part}
                </span>
            );
        });
    };

    const tokens: React.ReactNode[] = [];
    let match;
    let tokenIdx = 0;

    while ((match = tokenRegex.exec(content)) !== null) {
        const token = match[0];
        const keyPrefix = `tok-${tokenIdx++}`;

        if (/^"(\\u[a-zA-Z0-9]{4}|\\[^u]|[^\\"])*"\s*:$/.test(token)) {
            // JSON Property Key (e.g. "model": or "messages":)
            const colonIndex = token.lastIndexOf(':');
            const keyStr = token.slice(0, colonIndex);
            const colonStr = token.slice(colonIndex);
            tokens.push(
                <React.Fragment key={keyPrefix}>
                    {renderTextWithSearch(keyStr, 'text-sky-600 dark:text-sky-300 font-medium', `${keyPrefix}-k`)}
                    {renderTextWithSearch(colonStr, 'text-slate-400 dark:text-slate-500', `${keyPrefix}-c`)}
                </React.Fragment>
            );
        } else if (/^"(\\u[a-zA-Z0-9]{4}|\\[^u]|[^\\"])*"$/.test(token)) {
            // String Literal value
            tokens.push(renderTextWithSearch(token, 'text-emerald-700 dark:text-emerald-300', keyPrefix));
        } else if (/^(true|false)$/.test(token)) {
            // Boolean value
            tokens.push(renderTextWithSearch(token, 'text-purple-600 dark:text-purple-400 font-semibold', keyPrefix));
        } else if (token === 'null') {
            // Null value
            tokens.push(renderTextWithSearch(token, 'text-rose-500 dark:text-rose-400 font-semibold italic', keyPrefix));
        } else if (/^-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?$/.test(token)) {
            // Number value
            tokens.push(renderTextWithSearch(token, 'text-amber-600 dark:text-amber-300 font-semibold', keyPrefix));
        } else if (/^[{}[\],:]$/.test(token)) {
            // Structural punctuation
            tokens.push(renderTextWithSearch(token, 'text-slate-400 dark:text-slate-500', keyPrefix));
        } else {
            // Whitespace or plain fallback text
            tokens.push(renderTextWithSearch(token, 'text-slate-700 dark:text-slate-300', keyPrefix));
        }
    }

    return (
        <pre className="text-[11px] font-mono whitespace-pre-wrap select-text leading-relaxed m-0 p-0 font-normal">
            {tokens}
        </pre>
    );
};

const PayloadViewerCard: React.FC<PayloadViewerCardProps> = ({
    cardId,
    title,
    badge,
    badgeStyle,
    rawPayload,
    concisePayload,
    headersJson,
    viewMode,
    emptyPlaceholder,
    onCopy,
    isCopied,
}) => {
    const { t } = useTranslation();
    const [searchTerm, setSearchTerm] = useState('');
    const [currentMatchIndex, setCurrentMatchIndex] = useState(0);
    const containerRef = useRef<HTMLDivElement>(null);

    const activeContent = useMemo(() => {
        if (viewMode === 'concise') {
            return concisePayload || rawPayload || '';
        }
        return rawPayload || '';
    }, [viewMode, concisePayload, rawPayload]);

    const formattedContent = useMemo(() => {
        if (!activeContent) return '';
        try {
            const obj = JSON.parse(activeContent);
            return JSON.stringify(obj, null, 2);
        } catch {
            return activeContent;
        }
    }, [activeContent]);

    const matchesCount = useMemo(() => {
        if (!searchTerm.trim() || !formattedContent) return 0;
        try {
            const escaped = searchTerm.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
            const matches = formattedContent.match(new RegExp(escaped, 'gi'));
            return matches ? matches.length : 0;
        } catch {
            return 0;
        }
    }, [searchTerm, formattedContent]);

    useEffect(() => {
        setCurrentMatchIndex(0);
    }, [searchTerm, viewMode]);

    useEffect(() => {
        if (searchTerm.trim() && matchesCount > 0 && containerRef.current) {
            const activeEl = containerRef.current.querySelector(`#active-match-${cardId}`);
            if (activeEl) {
                activeEl.scrollIntoView({ behavior: 'smooth', block: 'center' });
            }
        }
    }, [currentMatchIndex, searchTerm, matchesCount, cardId]);

    const handleNext = () => {
        if (matchesCount > 0) {
            setCurrentMatchIndex((prev) => (prev + 1) % matchesCount);
        }
    };

    const handlePrev = () => {
        if (matchesCount > 0) {
            setCurrentMatchIndex((prev) => (prev - 1 + matchesCount) % matchesCount);
        }
    };

    const renderBody = () => {
        if (!formattedContent) {
            return (
                <div className="flex-1 flex flex-col items-center justify-center p-8 text-center text-slate-400 dark:text-slate-500 select-none">
                    <span className="text-xs italic">{emptyPlaceholder}</span>
                </div>
            );
        }

        return renderHighlightedJson(formattedContent, searchTerm, currentMatchIndex, cardId);
    };

    const searchInputRef = useRef<HTMLInputElement>(null);

    const prettyHeaders = useMemo(() => {
        if (!headersJson) return '';
        try {
            return JSON.stringify(JSON.parse(headersJson), null, 2);
        } catch {
            return headersJson;
        }
    }, [headersJson]);

    const copyPayload = prettyHeaders
        ? `/* headers */\n${prettyHeaders}\n\n/* body */\n${formattedContent}`
        : formattedContent;

    return (
        <div
            className="payload-viewer-card flex flex-col h-full bg-slate-50/50 dark:bg-[#0d1117] rounded-xl border border-slate-200 dark:border-slate-800 overflow-hidden shadow-sm outline-none"
            tabIndex={-1}
            onKeyDown={(e) => {
                if ((e.ctrlKey || e.metaKey) && (e.key === 'f' || e.key === 'F')) {
                    e.preventDefault();
                    e.stopPropagation();
                    searchInputRef.current?.focus();
                    searchInputRef.current?.select();
                }
            }}
        >
            {/* Card Header */}
            <div className="px-3.5 py-2 border-b border-slate-200 dark:border-slate-800/80 bg-white/95 dark:bg-[#161b22] flex items-center justify-between gap-2 shrink-0">
                <div className="flex items-center gap-2 min-w-0">
                    <span className={`px-2 py-0.5 rounded text-[10px] font-black uppercase tracking-wider border shrink-0 ${badgeStyle}`}>
                        {badge}
                    </span>
                    <h3 className="text-xs font-bold text-slate-800 dark:text-slate-200 truncate" title={title}>
                        {title}
                    </h3>
                </div>

                <div className="flex items-center gap-1 shrink-0">
                    <button
                        type="button"
                        onClick={() => onCopy(copyPayload)}
                        disabled={!formattedContent && !prettyHeaders}
                        className="btn btn-ghost btn-xs gap-1 h-7 px-2 text-slate-600 dark:text-slate-300 hover:bg-slate-100 dark:hover:bg-slate-800"
                        title={isCopied ? t('proxy.config.btn_copied', '已复制') : t('proxy.config.btn_copy', '复制')}
                    >
                        {isCopied ? <CheckCircle size={12} className="text-emerald-500" /> : <Copy size={12} />}
                        <span className="text-[10px] font-medium">{isCopied ? t('proxy.config.btn_copied', '已复制') : t('proxy.config.btn_copy', '复制')}</span>
                    </button>
                </div>
            </div>

            {/* In-block Search Bar */}
            <div className="px-2.5 py-1.5 bg-slate-100/70 dark:bg-[#161b22]/80 border-b border-slate-200 dark:border-slate-800/80 flex items-center gap-1.5 shrink-0">
                <div className="relative flex-1 min-w-0 flex items-center">
                    <Search size={12} className="absolute left-2 text-slate-400 pointer-events-none" />
                    <input
                        ref={searchInputRef}
                        type="text"
                        placeholder={t('monitor.details.search_placeholder', '搜索此报文... (Enter下个, Shift+Enter上个)')}
                        value={searchTerm}
                        onChange={(e) => setSearchTerm(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === 'Enter') {
                                e.preventDefault();
                                if (e.shiftKey) {
                                    handlePrev();
                                } else {
                                    handleNext();
                                }
                            } else if ((e.ctrlKey || e.metaKey) && (e.key === 'f' || e.key === 'F')) {
                                e.preventDefault();
                                e.stopPropagation();
                                searchInputRef.current?.select();
                            }
                        }}
                        className="input input-xs input-bordered w-full pl-6 pr-6 text-[11px] h-7 bg-white dark:bg-[#0d1117] border-slate-200 dark:border-slate-700/80 text-slate-800 dark:text-slate-200 rounded-md focus:border-blue-500"
                    />
                    {searchTerm && (
                        <button
                            type="button"
                            onClick={() => setSearchTerm('')}
                            className="absolute right-1.5 text-slate-400 hover:text-slate-600 dark:hover:text-slate-200 p-0.5"
                            title="清除搜索"
                        >
                            <X size={12} />
                        </button>
                    )}
                </div>

                {/* Match Counter and Next/Prev Navigation */}
                {searchTerm.trim() && (
                    <div className="flex items-center gap-1 shrink-0 bg-white dark:bg-[#0d1117] border border-slate-200 dark:border-slate-700/80 rounded-md px-1.5 py-0.5 h-7">
                        <span className="text-[10px] font-mono font-semibold text-slate-600 dark:text-slate-300">
                            {matchesCount > 0 ? `${currentMatchIndex + 1}/${matchesCount}` : '0 匹配'}
                        </span>
                        <div className="flex items-center">
                            <button
                                type="button"
                                onClick={handlePrev}
                                disabled={matchesCount <= 1}
                                className="btn btn-ghost btn-xs p-0.5 h-5 min-h-0 text-slate-500 dark:text-slate-400 disabled:opacity-30"
                                title="上一处 (Shift+Enter)"
                            >
                                <ChevronUp size={12} />
                            </button>
                            <button
                                type="button"
                                onClick={handleNext}
                                disabled={matchesCount <= 1}
                                className="btn btn-ghost btn-xs p-0.5 h-5 min-h-0 text-slate-500 dark:text-slate-400 disabled:opacity-30"
                                title="下一处 (Enter)"
                            >
                                <ChevronDown size={12} />
                            </button>
                        </div>
                    </div>
                )}
            </div>

            {/* Scrollable Content Body */}
            <div
                ref={containerRef}
                tabIndex={0}
                className="flex-1 overflow-y-auto overflow-x-auto p-3.5 bg-slate-50/40 dark:bg-[#0d1117] font-mono text-[11px] outline-none focus:ring-1 focus:ring-blue-500/20"
            >
                {prettyHeaders && (
                    <div className="mb-3 rounded-lg overflow-hidden border border-slate-200 dark:border-slate-800/80 bg-slate-100/50 dark:bg-slate-900/40">
                        <div className="px-2.5 py-1 bg-slate-200/60 dark:bg-[#161b22] border-b border-slate-200 dark:border-slate-800/80 flex items-center justify-between">
                            <span className="text-[10px] font-mono font-bold uppercase tracking-wider text-slate-500 dark:text-slate-400">
                                {t('monitor.details.headers', 'Headers')}
                            </span>
                            <span className="text-[9px] font-mono text-slate-400 dark:text-slate-500">HTTP Headers</span>
                        </div>
                        <div className="p-2.5 font-mono text-[11px] leading-relaxed">
                            {renderHighlightedJson(prettyHeaders, '', 0, `${cardId}-hdr`)}
                        </div>
                    </div>
                )}
                {renderBody()}
            </div>
        </div>
    );
};

export const ProxyMonitor: React.FC<ProxyMonitorProps> = ({ className }) => {
    const { t } = useTranslation();
    const [logs, setLogs] = useState<ProxyRequestLog[]>([]);
    const [stats, setStats] = useState<ProxyStats>({ total_requests: 0, success_count: 0, error_count: 0 });
    const [filter, setFilter] = useState('');
    const [accountFilter, setAccountFilter] = useState('');
    // [FIX] 使用 ref 存储最新的筛选条件，避免 setInterval 闭包问题
    const filterRef = useRef(filter);
    const accountFilterRef = useRef(accountFilter);
    const currentPageRef = useRef(1);
    const globalFilterInputRef = useRef<HTMLInputElement>(null);
    const [selectedLog, setSelectedLog] = useState<ProxyRequestLog | null>(null);
    const [isLoggingEnabled, setIsLoggingEnabled] = useState(false);
    const [isClearConfirmOpen, setIsClearConfirmOpen] = useState(false);
    const [payloadViewMode, setPayloadViewMode] = useState<'concise' | 'full'>('concise');
    const [showMetadata, setShowMetadata] = useState(true);
    const [copiedCard, setCopiedCard] = useState<string | null>(null);

    // 全局快捷键 Ctrl+F：当焦点在报文卡片之外时，聚焦主界面的全局过滤搜索框
    useEffect(() => {
        const handleGlobalKeyDown = (e: KeyboardEvent) => {
            if ((e.ctrlKey || e.metaKey) && (e.key === 'f' || e.key === 'F')) {
                const activeEl = document.activeElement;
                if (activeEl && activeEl.closest('.payload-viewer-card')) {
                    return;
                }
                e.preventDefault();
                globalFilterInputRef.current?.focus();
                globalFilterInputRef.current?.select();
            }
        };
        window.addEventListener('keydown', handleGlobalKeyDown);
        return () => window.removeEventListener('keydown', handleGlobalKeyDown);
    }, []);

    const conciseRequestBody = useMemo(() => {
        return selectedLog?.request_body
            ? extractConcisePayload(selectedLog.request_body, 'request', selectedLog)
            : '';
    }, [selectedLog?.request_body, selectedLog?.id]);

    const conciseUpstreamBody = useMemo(() => {
        return selectedLog?.upstream_request_body
            ? extractConcisePayload(selectedLog.upstream_request_body, 'upstream', selectedLog)
            : '';
    }, [selectedLog?.upstream_request_body, selectedLog?.id]);

    const conciseResponseBody = useMemo(() => {
        return selectedLog?.response_body
            ? extractConcisePayload(selectedLog.response_body, 'response', selectedLog)
            : '';
    }, [selectedLog?.response_body, selectedLog?.id, selectedLog?.input_tokens, selectedLog?.output_tokens, selectedLog?.cached_tokens]);

    const { accounts, fetchAccounts } = useAccountStore();

    // Pagination state
    const PAGE_SIZE_OPTIONS = [50, 100, 200, 500];
    const [pageSize, setPageSize] = useState(100);
    const [currentPage, setCurrentPage] = useState(1);
    const [totalCount, setTotalCount] = useState(0);
    const [loading, setLoading] = useState(false);
    const [loadingDetail, setLoadingDetail] = useState(false);

    const uniqueAccounts = useMemo(() => {
        const emailSet = new Set<string>();
        logs.forEach(log => {
            if (log.account_email) {
                emailSet.add(log.account_email);
            }
        });
        accounts.forEach(acc => {
            emailSet.add(acc.email);
        });
        return Array.from(emailSet).sort();
    }, [logs, accounts]);

    const loadData = async (page = 1, searchFilter = filter, accountEmailFilter = accountFilter) => {
        if (loading) return;
        setLoading(true);

        try {
            // Add timeout control (10 seconds)
            const timeoutPromise = new Promise((_, reject) =>
                setTimeout(() => reject(new Error('Request timeout')), 10000)
            );

            const config = await Promise.race([
                invoke<AppConfig>('load_config'),
                timeoutPromise
            ]) as AppConfig;

            if (config && config.proxy) {
                setIsLoggingEnabled(config.proxy.enable_logging);
                await invoke('set_proxy_monitor_enabled', { enabled: config.proxy.enable_logging });
            }

            const errorsOnly = searchFilter === '__ERROR__';
            const baseFilter = errorsOnly ? '' : searchFilter;
            const actualFilter = accountEmailFilter
                ? (baseFilter ? `${baseFilter} ${accountEmailFilter}` : accountEmailFilter)
                : baseFilter;

            // Get count with filter
            const count = await Promise.race([
                invoke<number>('get_proxy_logs_count_filtered', {
                    filter: actualFilter,
                    errorsOnly: errorsOnly
                }),
                timeoutPromise
            ]) as number;
            setTotalCount(count);

            // Use filtered paginated query
            const offset = (page - 1) * pageSize;
            const history = await Promise.race([
                invoke<ProxyRequestLog[]>('get_proxy_logs_filtered', {
                    filter: actualFilter,
                    errorsOnly: errorsOnly,
                    limit: pageSize,
                    offset: offset
                }),
                timeoutPromise
            ]) as ProxyRequestLog[];

            if (Array.isArray(history)) {
                setLogs(history);
                // Clear pending logs to avoid duplicates (database data is authoritative)
                pendingLogsRef.current = [];
            }

            const currentStats = await Promise.race([
                invoke<ProxyStats>('get_proxy_stats'),
                timeoutPromise
            ]) as ProxyStats;

            if (currentStats) setStats(currentStats);
        } catch (e: any) {
            console.error("Failed to load proxy data", e);
            if (e.message === 'Request timeout') {
                // Show timeout error to user
                console.error('Loading monitor data timeout, please try again later');
            }
        } finally {
            setLoading(false);
        }
    };

    const totalPages = Math.ceil(totalCount / pageSize);
    const pageStart = totalCount === 0 ? 0 : (currentPage - 1) * pageSize + 1;
    const pageEnd = totalCount === 0 ? 0 : Math.min(currentPage * pageSize, totalCount);

    const goToPage = (page: number) => {
        if (page >= 1 && page <= totalPages && page !== currentPage) {
            setCurrentPage(page);
            currentPageRef.current = page; // [FIX] 同步 ref
            loadData(page, filter, accountFilter);
        }
    };

    const toggleLogging = async () => {
        const newState = !isLoggingEnabled;
        try {
            const config = await invoke<AppConfig>('load_config');
            if (config && config.proxy) {
                config.proxy.enable_logging = newState;
                await invoke('save_config', { config });
                await invoke('set_proxy_monitor_enabled', { enabled: newState });
                setIsLoggingEnabled(newState);
            }
        } catch (e) {
            console.error("Failed to toggle logging", e);
        }
    };

    const pendingLogsRef = useRef<ProxyRequestLog[]>([]);
    const listenerSetupRef = useRef(false);
    const isMountedRef = useRef(true);

    useEffect(() => {
        isMountedRef.current = true;
        loadData();
        fetchAccounts();

        let unlistenFn: (() => void) | null = null;
        let updateTimeout: number | null = null;

        const setupListener = async () => {
            if (!isTauri()) return;
            // Prevent duplicate listener registration (React 18 StrictMode)
            if (listenerSetupRef.current) {
                console.debug('[ProxyMonitor] Listener already set up, skipping...');
                return;
            }
            listenerSetupRef.current = true;

            console.debug('[ProxyMonitor] Setting up event listener for proxy://request');
            unlistenFn = await listen<ProxyRequestLog>('proxy://request', (event) => {
                if (!isMountedRef.current) return;

                const newLog = event.payload;

                // 移除 body 以减少内存占用
                const logSummary = {
                    ...newLog,
                    request_body: undefined,
                    upstream_request_body: undefined,
                    response_body: undefined
                };

                // Check if this log already exists (deduplicate at event level)
                const alreadyExists = pendingLogsRef.current.some(log => log.id === newLog.id);
                if (alreadyExists) {
                    console.debug('[ProxyMonitor] Duplicate event ignored:', newLog.id);
                    return;
                }

                pendingLogsRef.current.push(logSummary);

                // 防抖:每 500ms 批量更新一次
                if (updateTimeout) clearTimeout(updateTimeout);
                updateTimeout = window.setTimeout(async () => {
                    if (!isMountedRef.current) return;

                    const currentPending = pendingLogsRef.current;
                    if (currentPending.length > 0) {
                        setLogs(prev => {
                            // Deduplicate by id
                            const existingIds = new Set(prev.map(log => log.id));
                            const uniqueNewLogs = currentPending.filter(log => !existingIds.has(log.id));
                            // Merge and sort by timestamp descending (newest first)
                            const merged = [...uniqueNewLogs, ...prev];
                            merged.sort((a, b) => b.timestamp - a.timestamp);
                            return merged.slice(0, 100);
                        });

                        // Fetch stats and total count from backend instead of local calculation
                        try {
                            const [currentStats, count] = await Promise.all([
                                invoke<ProxyStats>('get_proxy_stats'),
                                invoke<number>('get_proxy_logs_count_filtered', { filter: '', errorsOnly: false })
                            ]);
                            if (isMountedRef.current) {
                                if (currentStats) setStats(currentStats);
                                setTotalCount(count);
                            }
                        } catch (e) {
                            console.error('Failed to fetch stats:', e);
                        }

                        pendingLogsRef.current = [];
                    }
                }, 500);
            });
        };
        setupListener();

        // Web 模式補強：如果不是 Tauri 環境，則啟用定時輪詢
        let pollInterval: number | null = null;
        if (!isTauri()) {
            console.debug('[ProxyMonitor] Web mode detected, starting auto-poll (10s)');
            pollInterval = window.setInterval(() => {
                if (isMountedRef.current && !loading) {
                    // [FIX] 使用 ref.current 获取最新的筛选条件
                    loadData(currentPageRef.current, filterRef.current, accountFilterRef.current);
                }
            }, 10000);
        }

        return () => {
            isMountedRef.current = false;
            listenerSetupRef.current = false;
            if (unlistenFn) unlistenFn();
            if (updateTimeout) clearTimeout(updateTimeout);
            if (pollInterval) clearInterval(pollInterval);
        };
    }, []);

    useEffect(() => {
        setCopiedCard(null);
    }, [selectedLog?.id]);

    // Reload when pageSize changes
    useEffect(() => {
        setCurrentPage(1);
        loadData(1, filter, accountFilter);
    }, [pageSize]);

    // Reload when filter changes (search based on all logs)
    useEffect(() => {
        setCurrentPage(1);
        loadData(1, filter, accountFilter);
        // [FIX] 同步 ref 值，供 setInterval 使用
        filterRef.current = filter;
        accountFilterRef.current = accountFilter;
        currentPageRef.current = 1;
    }, [filter, accountFilter]);

    // Logs are already filtered and sorted by backend
    // Apply account filter on frontend
    const filteredLogs = useMemo(() => {
        if (!accountFilter) return logs;
        return logs.filter(log => log.account_email === accountFilter);
    }, [logs, accountFilter]);

    const quickFilters = [
        { label: t('monitor.filters.all'), value: '' },
        { label: 'claude', value: 'claude' },
        { label: 'flash', value: 'flash' },
        { label: 'pro', value: 'pro' },
        { label: 'agent', value: 'agent' },
        { label: t('monitor.filters.error'), value: '__ERROR__' },
        { label: t('monitor.filters.chat'), value: 'completions' },
        { label: t('monitor.filters.gemini'), value: 'gemini' },
        { label: t('monitor.filters.images'), value: 'images' }
    ];

    const clearLogs = () => {
        setIsClearConfirmOpen(true);
    };

    const executeClearLogs = async () => {
        setIsClearConfirmOpen(false);
        try {
            await invoke('clear_proxy_logs');
            setLogs([]);
            setStats({ total_requests: 0, success_count: 0, error_count: 0 });
            setTotalCount(0);
        } catch (e) {
            console.error("Failed to clear logs", e);
        }
    };




    return (
        <div className={`flex flex-col bg-white dark:bg-base-100 rounded-xl shadow-sm border border-gray-100 dark:border-base-200 overflow-hidden ${className || 'flex-1'}`}>
            <div className="p-3 border-b border-gray-100 dark:border-base-200 space-y-3 bg-gray-50/30 dark:bg-base-200/30">
                <div className="flex items-center gap-4">
                    <button
                        onClick={toggleLogging}
                        className={`btn btn-sm gap-2 px-4 border font-bold ${isLoggingEnabled
                            ? 'bg-red-500 border-red-600 text-white animate-pulse'
                            : 'bg-white dark:bg-base-200 border-gray-300 text-gray-600'
                            }`}
                    >
                        <div className={`w-2.5 h-2.5 rounded-full ${isLoggingEnabled ? 'bg-white' : 'bg-gray-400'}`} />
                        {isLoggingEnabled ? t('monitor.logging_status.active') : t('monitor.logging_status.paused')}
                    </button>

                    <div className="relative flex-1">
                        <Search className="absolute left-2.5 top-2 text-gray-400" size={14} />
                        <input
                            ref={globalFilterInputRef}
                            type="text"
                            placeholder={t('monitor.filters.placeholder')}
                            className="input input-sm input-bordered w-full pl-9 text-xs"
                            value={filter}
                            onChange={(e) => setFilter(e.target.value)}
                        />
                    </div>

                    <div className="relative">
                        <User className="absolute left-2.5 top-2 text-gray-400 z-10" size={14} />
                        <select
                            className="select select-sm select-bordered pl-8 text-xs min-w-[140px] max-w-[220px]"
                            value={accountFilter}
                            onChange={(e) => setAccountFilter(e.target.value)}
                            title={t('monitor.filters.by_account')}
                        >
                            <option value="">{t('monitor.filters.all_accounts')}</option>
                            {uniqueAccounts.map(email => (
                                <option key={email} value={email} title={email}>
                                    {email}
                                </option>
                            ))}
                        </select>
                    </div>

                    <div className="hidden lg:flex gap-4 text-[10px] font-bold uppercase">
                        <span className="text-blue-500">{formatCompactNumber(stats.total_requests)} {t('monitor.stats.total')}</span>
                        <span className="text-green-500">{formatCompactNumber(stats.success_count)} {t('monitor.stats.ok')}</span>
                        <span className="text-red-500">{formatCompactNumber(stats.error_count)} {t('monitor.stats.err')}</span>
                    </div>

                    <button onClick={() => loadData(currentPage, filter)} className="btn btn-sm btn-ghost text-gray-400" title={t('common.refresh')}>
                        <RefreshCw size={16} className={loading ? 'animate-spin' : ''} />
                    </button>
                    <button onClick={clearLogs} className="btn btn-sm btn-ghost text-gray-400">
                        <Trash2 size={16} />
                    </button>
                </div>

                <div className="flex flex-wrap items-center gap-2">
                    <span className="text-[10px] font-bold text-gray-400 uppercase">{t('monitor.filters.quick_filters')}</span>
                    {quickFilters.map(q => (
                        <button key={q.label} onClick={() => setFilter(q.value)} className={`px-2 py-0.5 rounded-full text-[10px] border ${filter === q.value ? 'bg-blue-500 text-white' : 'bg-white dark:bg-base-200 text-gray-500'}`}>
                            {q.label}
                        </button>
                    ))}
                    {(filter || accountFilter) && <button onClick={() => { setFilter(''); setAccountFilter(''); }} className="text-[10px] text-blue-500"> {t('monitor.filters.reset')} </button>}
                </div>
            </div>

            <LogTable
                logs={filteredLogs}
                loading={loading}
                onLogClick={async (log: ProxyRequestLog) => {
                    setLoadingDetail(true);
                    try {
                        const detail = await invoke<ProxyRequestLog>('get_proxy_log_detail', { logId: log.id });
                        setSelectedLog(detail);
                    } catch (e) {
                        console.error('Failed to load log detail', e);
                        setSelectedLog(log);
                    } finally {
                        setLoadingDetail(false);
                    }
                }}
                t={t}
            />

            {/* Pagination Controls */}
            <div className="flex items-center justify-between px-4 py-3 bg-gray-50 dark:bg-base-200 border-t border-gray-200 dark:border-base-300 text-xs">
                <div className="flex items-center gap-2 whitespace-nowrap">
                    <span className="text-gray-500">{t('common.per_page')}</span>
                    <select
                        value={pageSize}
                        onChange={(e) => setPageSize(Number(e.target.value))}
                        className="select select-xs select-bordered w-16"
                    >
                        {PAGE_SIZE_OPTIONS.map(size => (
                            <option key={size} value={size}>{size}</option>
                        ))}
                    </select>
                </div>

                <div className="flex items-center gap-3">
                    <button
                        onClick={() => goToPage(currentPage - 1)}
                        disabled={currentPage <= 1 || loading}
                        className="btn btn-xs btn-ghost"
                    >
                        <ChevronLeft size={14} />
                    </button>
                    <span className="text-gray-600 dark:text-gray-400 min-w-[80px] text-center">
                        {currentPage} / {totalPages || 1}
                    </span>
                    <button
                        onClick={() => goToPage(currentPage + 1)}
                        disabled={currentPage >= totalPages || loading}
                        className="btn btn-xs btn-ghost"
                    >
                        <ChevronRight size={14} />
                    </button>
                </div>

                <div className="text-gray-500">
                    {t('common.pagination_info', { start: pageStart, end: pageEnd, total: totalCount })}
                </div>
            </div>

            {selectedLog && (
                <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm p-2 sm:p-3 md:p-4" onClick={() => setSelectedLog(null)}>
                    <div className="bg-white dark:bg-[#161b22] rounded-2xl shadow-2xl w-full max-w-[98vw] xl:max-w-[1720px] h-[94vh] max-h-[94vh] flex flex-col overflow-hidden border border-slate-200 dark:border-slate-800" onClick={e => e.stopPropagation()}>
                        {/* Modal Header */}
                        <div className="px-4 py-2.5 border-b border-slate-200 dark:border-slate-800 flex items-center justify-between bg-slate-50 dark:bg-[#161b22] shrink-0">
                            <div className="flex items-center gap-3 min-w-0">
                                {loadingDetail && <div className="loading loading-spinner loading-sm shrink-0"></div>}
                                <span className={`badge badge-sm text-white border-none font-bold shrink-0 ${selectedLog.status >= 200 && selectedLog.status < 400 ? 'badge-success' : 'badge-error'}`}>{selectedLog.status}</span>
                                <span className="font-mono font-bold text-slate-900 dark:text-slate-100 text-sm shrink-0">{selectedLog.method}</span>
                                <span className="text-xs text-slate-500 dark:text-slate-400 font-mono truncate max-w-lg hidden sm:inline" title={selectedLog.url}>{selectedLog.url}</span>
                            </div>
                            <button onClick={() => setSelectedLog(null)} className="btn btn-ghost btn-sm btn-circle text-slate-500 dark:text-slate-400 hover:bg-slate-200 dark:hover:bg-slate-800" aria-label="关闭"><X size={18} /></button>
                        </div>

                        {/* Modal Content */}
                        <div className="flex-1 min-h-0 flex flex-col p-3 sm:p-4 space-y-2.5 bg-slate-100/50 dark:bg-[#0a0e17] overflow-hidden">
                            {/* Metadata Section (Collapsible) */}
                            {showMetadata && (
                                <div className="bg-white dark:bg-[#161b22] p-3 sm:p-3.5 rounded-xl border border-slate-200 dark:border-slate-800/90 shadow-sm shrink-0 text-xs transition-all duration-200">
                                    <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-3">
                                        <div>
                                            <span className="block text-slate-400 dark:text-slate-500 uppercase font-black text-[9px] tracking-wider">{t('monitor.details.time')}</span>
                                            <span className="font-mono font-semibold text-slate-800 dark:text-slate-200 text-[11px] truncate block" title={new Date(selectedLog.timestamp).toLocaleString()}>{new Date(selectedLog.timestamp).toLocaleString()}</span>
                                        </div>
                                        <div>
                                            <span className="block text-slate-400 dark:text-slate-500 uppercase font-black text-[9px] tracking-wider">{t('monitor.details.duration')}</span>
                                            <span className="font-mono font-semibold text-slate-800 dark:text-slate-200 text-[11px]">{selectedLog.duration}ms</span>
                                        </div>
                                        <div>
                                            <span className="block text-slate-400 dark:text-slate-500 uppercase font-black text-[9px] tracking-wider">{t('monitor.details.tokens')}</span>
                                            <div className="font-mono text-[10px] flex items-center gap-1.5 mt-0.5">
                                                {(() => {
                                                    const totalIn = (selectedLog.cached_tokens && selectedLog.cached_tokens > (selectedLog.input_tokens ?? 0))
                                                        ? (selectedLog.input_tokens ?? 0) + selectedLog.cached_tokens
                                                        : (selectedLog.input_tokens ?? 0);
                                                    return (
                                                        <span className="text-blue-700 dark:text-blue-300 bg-blue-100 dark:bg-blue-950/60 px-1.5 py-0.5 rounded font-bold" title={`Total Input Tokens: ${totalIn}`}>
                                                            In: {formatCompactNumber(totalIn)}
                                                        </span>
                                                    );
                                                })()}
                                                <span className="text-emerald-700 dark:text-emerald-300 bg-emerald-100 dark:bg-emerald-950/60 px-1.5 py-0.5 rounded font-bold">Out: {formatCompactNumber(selectedLog.output_tokens ?? 0)}</span>
                                                {selectedLog.cached_tokens != null && selectedLog.cached_tokens > 0 && (
                                                    <span className="text-purple-700 dark:text-purple-300 bg-purple-100 dark:bg-purple-950/60 px-1.5 py-0.5 rounded font-bold">Cache: {formatCompactNumber(selectedLog.cached_tokens)}</span>
                                                )}
                                            </div>
                                        </div>
                                        <div>
                                            <span className="block text-slate-400 dark:text-slate-500 uppercase font-black text-[9px] tracking-wider">{t('monitor.details.protocol')}</span>
                                            <span className={`inline-block px-1.5 py-0.5 rounded font-mono font-black text-[10px] uppercase mt-0.5 ${
                                                selectedLog.protocol === 'openai' ? 'bg-emerald-100 text-emerald-700 dark:bg-emerald-950/70 dark:text-emerald-300 border border-emerald-200 dark:border-emerald-800/60' :
                                                selectedLog.protocol === 'anthropic' ? 'bg-orange-100 text-orange-700 dark:bg-orange-950/70 dark:text-orange-300 border border-orange-200 dark:border-orange-800/60' :
                                                selectedLog.protocol === 'gemini' ? 'bg-blue-100 text-blue-700 dark:bg-blue-950/70 dark:text-blue-300 border border-blue-200 dark:border-blue-800/60' :
                                                'bg-slate-100 text-slate-700 dark:bg-slate-900/60 dark:text-slate-300'
                                            }`}>
                                                {selectedLog.protocol || '-'}
                                            </span>
                                        </div>
                                        <div>
                                            <span className="block text-slate-400 dark:text-slate-500 uppercase font-black text-[9px] tracking-wider">{t('monitor.details.model')}</span>
                                            <span className="font-mono font-bold text-blue-600 dark:text-blue-400 truncate block text-[11px]" title={selectedLog.model}>{selectedLog.model || '-'}</span>
                                            {selectedLog.mapped_model && selectedLog.model !== selectedLog.mapped_model && (
                                                <span className="font-mono text-emerald-600 dark:text-emerald-400 truncate block text-[10px]" title={selectedLog.mapped_model}>➔ {selectedLog.mapped_model}</span>
                                            )}
                                        </div>
                                        <div>
                                            <span className="block text-slate-400 dark:text-slate-500 uppercase font-black text-[9px] tracking-wider">{t('monitor.details.account_used')}</span>
                                            <span className="font-mono text-slate-800 dark:text-slate-200 truncate block text-[11px]" title={selectedLog.account_email || '-'}>{selectedLog.account_email || '-'}</span>
                                        </div>
                                    </div>
                                </div>
                            )}

                            {/* Mode & Toolbar Bar */}
                            <div className="flex flex-wrap items-center justify-between gap-2 px-1 shrink-0">
                                <div className="flex items-center gap-2">
                                    <div className="inline-flex items-center p-1 bg-slate-200/70 dark:bg-[#161b22] rounded-xl border border-slate-300/70 dark:border-slate-800 gap-1 shadow-inner">
                                        <button
                                            type="button"
                                            onClick={() => setPayloadViewMode('concise')}
                                            className={`flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold transition-all duration-150 cursor-pointer select-none ${
                                                payloadViewMode === 'concise'
                                                    ? 'bg-white dark:bg-[#21262d] text-blue-600 dark:text-blue-400 shadow-sm border border-slate-200 dark:border-slate-700'
                                                    : 'text-slate-500 dark:text-slate-400 hover:text-slate-900 dark:hover:text-slate-200'
                                            }`}
                                        >
                                            <Sparkles size={13} className={payloadViewMode === 'concise' ? 'text-blue-600 dark:text-blue-400' : 'text-slate-400'} />
                                            <span>{t('monitor.details.concise_mode', '简要模式')}</span>
                                        </button>
                                        <button
                                            type="button"
                                            onClick={() => setPayloadViewMode('full')}
                                            className={`flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold transition-all duration-150 cursor-pointer select-none ${
                                                payloadViewMode === 'full'
                                                    ? 'bg-white dark:bg-[#21262d] text-blue-600 dark:text-blue-400 shadow-sm border border-slate-200 dark:border-slate-700'
                                                    : 'text-slate-500 dark:text-slate-400 hover:text-slate-900 dark:hover:text-slate-200'
                                            }`}
                                        >
                                            <FileCode2 size={13} className={payloadViewMode === 'full' ? 'text-blue-600 dark:text-blue-400' : 'text-slate-400'} />
                                            <span>{t('monitor.details.full_mode', '完整模式')}</span>
                                        </button>
                                    </div>
                                    <span className="hidden sm:inline-block text-[11px] text-slate-500 dark:text-slate-400">
                                        {payloadViewMode === 'concise'
                                            ? t('monitor.details.concise_desc', '已为您精简工具参数与冗余字段，突出思考块、用量与对话主体')
                                            : '显示原始完整未修剪报文'}
                                    </span>
                                </div>

                                <div className="flex items-center gap-2">
                                    <button
                                        type="button"
                                        onClick={() => setShowMetadata((prev) => !prev)}
                                        className="btn btn-xs btn-ghost text-slate-500 dark:text-slate-400 hover:bg-slate-200 dark:hover:bg-slate-800 gap-1 text-[11px]"
                                        title={showMetadata ? '折叠元数据以增大报文视野' : '展开元数据信息'}
                                    >
                                        {showMetadata ? <EyeOff size={13} /> : <Eye size={13} />}
                                        <span>{showMetadata ? '收起元数据' : '展开元数据'}</span>
                                    </button>
                                </div>
                            </div>

                            {/* Horizontal 3-Column Grid */}
                            <div className="grid grid-cols-1 lg:grid-cols-3 gap-3 flex-1 min-h-0 overflow-hidden">
                                <PayloadViewerCard
                                    cardId="req"
                                    title={t('monitor.details.request_payload', '请求报文 (Request)')}
                                    badge="REQUEST"
                                    badgeStyle="bg-blue-50 text-blue-700 dark:bg-blue-950/70 dark:text-blue-300 border-blue-200 dark:border-blue-800/60"
                                    rawPayload={selectedLog.request_body}
                                    concisePayload={conciseRequestBody}
                                    headersJson={selectedLog.request_headers}
                                    viewMode={payloadViewMode}
                                    emptyPlaceholder={t('monitor.details.payload_empty', '无请求报文')}
                                    onCopy={async (text) => {
                                        const success = await copyToClipboard(text);
                                        if (success) {
                                            setCopiedCard('req');
                                            setTimeout(() => setCopiedCard(null), 2000);
                                        }
                                    }}
                                    isCopied={copiedCard === 'req'}
                                />
                                <PayloadViewerCard
                                    cardId="upstream"
                                    title={t('monitor.details.upstream_request_payload', '中转报文 (Forwarded)')}
                                    badge="FORWARDED"
                                    badgeStyle="bg-amber-50 text-amber-700 dark:bg-amber-950/70 dark:text-amber-300 border-amber-200 dark:border-amber-800/60"
                                    rawPayload={selectedLog.upstream_request_body}
                                    concisePayload={conciseUpstreamBody}
                                    headersJson={selectedLog.upstream_request_headers}
                                    viewMode={payloadViewMode}
                                    emptyPlaceholder={t('monitor.details.no_upstream_payload', '无中转报文 (直接转发或未记录)')}
                                    onCopy={async (text) => {
                                        const success = await copyToClipboard(text);
                                        if (success) {
                                            setCopiedCard('upstream');
                                            setTimeout(() => setCopiedCard(null), 2000);
                                        }
                                    }}
                                    isCopied={copiedCard === 'upstream'}
                                />
                                <PayloadViewerCard
                                    cardId="resp"
                                    title={t('monitor.details.response_payload', '响应报文 (Response)')}
                                    badge="RESPONSE"
                                    badgeStyle="bg-emerald-50 text-emerald-700 dark:bg-emerald-950/70 dark:text-emerald-300 border-emerald-200 dark:border-emerald-800/60"
                                    rawPayload={selectedLog.response_body}
                                    concisePayload={conciseResponseBody}
                                    headersJson={selectedLog.response_headers}
                                    viewMode={payloadViewMode}
                                    emptyPlaceholder={t('monitor.details.payload_empty', '无响应报文')}
                                    onCopy={async (text) => {
                                        const success = await copyToClipboard(text);
                                        if (success) {
                                            setCopiedCard('resp');
                                            setTimeout(() => setCopiedCard(null), 2000);
                                        }
                                    }}
                                    isCopied={copiedCard === 'resp'}
                                />
                            </div>
                        </div>
                    </div>
                </div>
            )}

            <ModalDialog
                isOpen={isClearConfirmOpen}
                title={t('monitor.dialog.clear_title')}
                message={t('monitor.dialog.clear_msg')}
                type="confirm"
                confirmText={t('common.delete')}
                isDestructive={true}
                onConfirm={executeClearLogs}
                onCancel={() => setIsClearConfirmOpen(false)}
            />
        </div>
    );
};
