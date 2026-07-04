from __future__ import annotations

from bibi_work_agent.runtime.memory_candidates import (
    MemoryCandidateCollector,
    extract_memory_candidates,
)


def test_extract_memory_candidates_from_structured_payload() -> None:
    candidates = extract_memory_candidates(
        {
            "payload": {
                "memory_candidates": [
                    {
                        "content": "销售额口径应使用净收入，不含税费。",
                        "layer": "semantic",
                        "confidence": 1.4,
                        "visibility": "private",
                    },
                    {"text": "x"},
                ]
            }
        }
    )

    assert candidates == [
        {
            "content": "销售额口径应使用净收入，不含税费。",
            "layer": "semantic",
            "confidence": 1.0,
            "visibility": "private",
        }
    ]


def test_extract_memory_candidates_from_json_text_and_section() -> None:
    assert extract_memory_candidates(
        '{"memory":{"candidates":[{"content":"用户偏好用中文总结销售分析。"}]}}'
    ) == [{"content": "用户偏好用中文总结销售分析。"}]

    candidates = extract_memory_candidates(
        """
Answer text.

Memory candidates:
- 用户每周一需要查看销售额周报。
- 销售额分析默认按区域拆分。
"""
    )

    assert candidates == [
        {"content": "用户每周一需要查看销售额周报。", "confidence": 0.55},
        {"content": "销售额分析默认按区域拆分。", "confidence": 0.55},
    ]


def test_collector_dedupes_and_skips_secret_like_content() -> None:
    collector = MemoryCandidateCollector()

    collector.observe(
        {
            "memory_candidates": [
                "销售额分析默认使用净收入。",
                " 销售额分析默认使用净收入。 ",
                "api_key=plain-secret should not be stored",
            ]
        }
    )

    assert collector.candidates() == [{"content": "销售额分析默认使用净收入。"}]
