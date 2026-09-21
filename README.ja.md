# OpenBNCT

[![CI](https://github.com/AvilaLabs/OpenBNCT/actions/workflows/ci.yml/badge.svg)](https://github.com/AvilaLabs/OpenBNCT/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![crates.io](https://img.shields.io/crates/v/openbnct-core.svg)](https://crates.io/crates/openbnct-core)
[![PyPI](https://img.shields.io/pypi/v/openbnct.svg)](https://pypi.org/project/openbnct/)
[![Status: early research](https://img.shields.io/badge/status-early_research-orange.svg)](docs/ROADMAP.md)
[![Clinical use: not validated](https://img.shields.io/badge/clinical_use-not_validated-red.svg)](docs/DISCLAIMER.md)

[English](README.md) | **日本語**

<p align="left">
  <a href="https://openbnct.avilalabs.org"><img src="https://img.shields.io/badge/%E3%83%96%E3%83%A9%E3%82%A6%E3%82%B6%E3%81%A7%E9%96%8B%E3%81%8F-openbnct.avilalabs.org-0d9488?style=for-the-badge" alt="ブラウザで開く"></a>
  <a href="https://github.com/AvilaLabs/OpenBNCT/releases/latest"><img src="https://img.shields.io/badge/%E3%83%80%E3%82%A6%E3%83%B3%E3%83%AD%E3%83%BC%E3%83%89-%E3%83%87%E3%82%B9%E3%82%AF%E3%83%88%E3%83%83%E3%83%97%E7%89%88-1f2937?style=for-the-badge" alt="デスクトップ版をダウンロード"></a>
</p>

*（旧称 NCTForge。クレート名・`openbnct` CLI/Python パッケージ・`openbnct.*` スキーマ識別子へ全面改称済みです。凍結済みベンチマーク証跡を含む改名前の `nctforge.*` アーティファクトは、コントラクト名前空間エイリアスにより引き続き読み込み可能です。[ARCHITECTURE.md](docs/ARCHITECTURE.md) を参照。）*

OpenBNCT は、ホウ素中性子捕捉療法（BNCT）の研究および独立検証を目的とした、
輸送コードに依存しない DICOM ネイティブのオープンソース・ワークベンチです。

本プロジェクトは Rust でゼロから開発されています。OpenMC は輸送コードに依存しない
境界の背後にある最初のバックエンドです。MCNP、PHITS などで外部計算された結果は、
公開された成分線量の相互交換コントラクトを通じて取り込めます。

> [!WARNING]
> OpenBNCT は初期研究段階のソフトウェアです。現時点では線量計算システムでも、
> 医療機器でもありません。臨床判断、治療計画、患者治療には使用できません。

## 目標

- DICOM CT および RT Structure Set の幾何学情報を厳密に検証する
- BNCT の4つの物理線量成分（ホウ素、窒素、水素反跳、光子）を分離して扱う
- 各ボクセルの統計的不確かさを保持する
- 物理輸送、ホウ素分布、生物学的重み付けを独立して確認できるようにする
- 入力、核データ、計算条件、出力を再現可能な証拠バンドルとして保存する
- OpenMC と外部輸送コードの結果を共通形式で比較する
- CLI、Python API、および egui デスクトップ・ワークベンチから同じ検証済みコアを使用する

## 現在の状況

- 合成 DICOM ベンチマーク `NF-BNCT-001` は凍結済みで、OpenMC バックエンドは
  デッキ生成・実行・statepoint からの線量バンドル収集まで実装済みです。
- 3つの凍結シードによる 600M ヒストリーの参照実行は、事前に宣言された全ての
  統計的合格基準（ROI 精度、ボクセル精度、推定量比較 291 件、シード間
  カイ二乗一貫性 378 件）を通過しました。合格レポートはリポジトリに
  コミットされています。
- ただし仕様上、独立実装の輸送コード（Geant4、またはライセンス取得済みの
  MCNP/PHITS ユーザーによる結果）による凍結ケースの再現が完了するまで、
  この候補は参照出力としては昇格しません。
- 生物学的解釈（加重モデル・分割照射・TCP/NTCP/UTCP・BED/EQD2）、線量体積
  指標、NIfTI I/O、MCNP/PHITS アダプター、CLI・GUI・Python の3面実装は
  すべて同一の Rust コントラクト上で稼働しています。
- さらに、施設ビーム記述と `beam qa` 品質評価、測定記録の比較インポート、
  DICOM RT Dose エクスポート、および OpenMC ウェイトウィンドウによる
  分散低減（`vr resolve`／`vr validate`）が実装されています。
- 内製の3次元マルチグループ S_N ソルバー（`sn solve`）が第二の輸送パスとして
  実装され、NF-BNCT-003 の解析オラクルに対して完全一致を確認済みです。
  随伴ソルバーによる CADIS/FW-CADIS ウェイトウィンドウ（`vr cadis`）、
  核データ共分散の伝播と Morris/Sobol 感度スクリーニング（`uq propagate`／
  `uq screen`）、輸送由来の線エネルギースペクトルの MKM 連携
  （`bio lineal-tally`）、ガンマ指数・変形オラクル・解析オラクル
  （`gamma`／`metamorphic`／`analytic`）、加速器由来ビームと BSA 層
  （`accelerator`／`bsa`）、ホウ素細胞内分布モデル
  （`boron microdistribution`）、OpenPINT 形式エクスポートと RTPLAN
  読み書き、ネイティブ PET DICOM の SUVbw 取り込み（`dicom import-pet`）、
  非負ビーム重みの決定論的最適化（`plan optimize`、
  `isoeffective` 目的量では埋め込み `BiologicalModel` による
  成分重み付き実効線量 `Σ_c w_c·D_c` を使用）、ビーム方向ごとの
  照野 sweep（`plan fields`、aim → ソルブ → 線量折り畳み →
  `openbnct.beam-field-set` マニフェスト）、および S_N ordinate
  sweep の rayon 並列化まで含みます。

開発段階と合格条件については [ロードマップ（英語）](docs/ROADMAP.md) を参照してください。

## 設計上の特徴

```text
DICOM／ケース入力
        |
輸送コードに依存しないケースモデル
        |
輸送アダプター（最初は OpenMC）
        |
4つの物理線量成分＋不確かさ
        |
バージョン管理された生物学的解釈
        |
品質保証、比較、可視化、証拠バンドル
```

OpenBNCT の中心的な役割は、特定施設の臨床 TPS を置き換えることではなく、研究コード、
輸送コード、施設間で BNCT の計算結果を再現・監査・比較できる公開基盤を提供することです。

## 関連資料

- [英語版 README](README.md)
- [開発ロードマップ](docs/ROADMAP.md)
- [技術ベースライン](docs/research/TECHNICAL_BASELINE.md)
- [アーキテクチャ](docs/ARCHITECTURE.md)
- [免責事項](docs/DISCLAIMER.md)
- [コントリビューションガイド](CONTRIBUTING.md)

日本語での Issue、技術的なフィードバック、用語・文書の改善提案も歓迎します。

## ライセンス

OpenBNCT は [Apache License 2.0](LICENSE) で公開されています。
