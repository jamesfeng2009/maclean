# Developer Certificate of Origin (DCO)

maclean 采用 **Developer Certificate of Origin**（开发者原创证明，版本 1.1）作为
贡献者来源认证机制。每一个提交（commit）必须包含 `Signed-off-by` 尾注，表示贡献者
确认了以下声明：

> By making a contribution to this project, I certify that:
>
> (a) The contribution was created in whole or in part by me and I have the right to
>     submit it under the open source license indicated in the file; or
>
> (b) The contribution is based upon previous work that, to the best of my knowledge,
>     is covered under an appropriate open source license and I have the right under
>     that license to submit that work with modifications, whether created in whole
>     or in part by me, under the same open source license (unless I am permitted to
>     submit under a different license); or
>
> (c) The contribution was provided directly to me by some other person who certified
>     (a), (b) or (c) and I have not modified it.
>
> (d) I understand and agree that this project and the contribution are public, and
>     that a record of the contribution (including all personal information I submit
>     with it, including my sign-off) is maintained indefinitely and may be
>     redistributed consistent with this project or the open source license(s)
>     involved.

## 如何签署

```bash
git commit -s -m "feat: ..."
# 或对已有 commit 补签：
git commit --amend -s
```

`git commit -s` 会自动追加：

```
Signed-off-by: Your Name <you@example.com>
```

## 检查

CI 会校验每个 commit 的 `Signed-off-by`（对应 `.github` 工作流与本地
`git log --check`）。未签署的提交会被要求补签后合并。
