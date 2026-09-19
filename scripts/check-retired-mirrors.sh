#!/usr/bin/env bash
# 已停用的国内源地址不许出现在代码与流水线里。
#
# 背景：国内源 2026-09-19 从腾讯云广州区改到香港区，广州区的镜像仓库与下载桶都已删除
# （plan/国内镜像与下载源.md 第一节）。地址写错不会报错，只会让国内用户永远拉不到镜像，
# 所以用一条 grep 把它钉死。
#
# 两个容易踩的点，都在这里处理了：
#   1. **广州是 `ccr.`，香港是 `hkccr.`，前者是后者的后缀** —— 直接 grep 会把正确的香港地址
#      也判成违规。所以先把带 hkccr 的行滤掉。
#   2. **守卫自己必须写出那几个违规字符串** —— 这份名单、以及 registry.rs 里那条
#      「候选源里不许出现广州地址」的单元测试，都得把它们原样写出来。
#      所以给了一个**逐行**的豁免标记 `retired-mirror-ok`：带这个标记的行跳过。
#      逐行而不是整文件豁免，是为了不把一整个文件变成盲区
#      （第一次把名单写在 ci.yml 里、整文件扫描，当场就把自己判违规了）。
#
# 只扫代码与配置，不扫 .md：成果文档里会**故意**提到这些地址（「广州区已停用」那几句说明）。
set -euo pipefail

cd "$(dirname "$0")/.."

RETIRED='ccr\.ccs\.tencentyun\.com|hunter-dl-1253756459|registry\.cn-hangzhou\.aliyuncs\.com'  # retired-mirror-ok（名单本身）

HITS=$(grep -rInE "$RETIRED" \
        --include='*.rs' --include='*.ts' --include='*.tsx' \
        --include='*.yml' --include='*.yaml' --include='*.json' --include='*.sh' \
        --exclude-dir=node_modules --exclude-dir=target --exclude-dir=.git . \
      | grep -v 'hkccr\.' \
      | grep -v 'retired-mirror-ok' || true)

if [ -n "$HITS" ]; then
  echo "::error::代码里出现了已停用的广州区 / 阿里云地址，见 plan/国内镜像与下载源.md"
  echo "$HITS"
  exit 1
fi
echo "OK：代码里没有已停用的国内源地址"
