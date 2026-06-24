//! **无障碍快照**(Accessibility snapshot)的**后端无关**核心。
//!
//! 把页面压成一棵 `role "name"` 的语义树 [`AxTree`]:用于**抗改版断言**(按角色+可见名定位)或
//! **喂给 LLM**(比整页 HTML 小一个数量级)。对标 Playwright `accessibility.snapshot`。
//!
//! 两条获取路径(都产 [`AxTree`],接口一致):
//! - **CDP 原生**(最准,仅 cdp):`Accessibility.getFullAXTree` 的**扁平**节点数组 → [`build_from_cdp`]
//!   按 `childIds` 重建成树(跳过 `ignored`)。见 `tab.ax_tree()`。
//! - **DOM 派生**(跨后端):页面里跑 [`AX_SNAPSHOT_JS`] 按 ARIA 规则算近似语义树 → [`build_from_snapshot`]。
//!   见 `tab.ax_snapshot()`(cdp / camoufox 都有)。
//!
//! 本模块只放**值类型 + 纯解析函数 + 注入脚本**(始终编译、可单测、跨后端复用)。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 无障碍树的一个节点(角色 + 可见名 + 可选值/描述/属性 + 子节点)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AxNode {
    /// 角色(如 `button`/`link`/`heading`/`textbox`)。
    pub role: String,
    /// 可见名(可访问名:aria-label / alt / placeholder / 文本…)。
    pub name: String,
    /// 当前值(输入框文本、滑块值等)。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub value: Option<String>,
    /// 描述(aria-description 等)。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<String>,
    /// 关键状态属性(如 `checked`/`disabled`/`expanded`/`focused`)。
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub properties: BTreeMap<String, String>,
    /// 子节点。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub children: Vec<AxNode>,
}

impl AxNode {
    /// 本节点是否"无信息"(无角色、无名、无属性)——用于裁剪/提升通用容器。
    fn is_blank(&self) -> bool {
        self.role.is_empty() && self.name.is_empty() && self.properties.is_empty()
    }

    /// 本节点及所有后代的总数。
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(AxNode::count).sum::<usize>()
    }

    /// 收集本子树所有节点引用(前序)。
    fn collect<'a>(&'a self, out: &mut Vec<&'a AxNode>) {
        out.push(self);
        for c in &self.children {
            c.collect(out);
        }
    }

    fn write_outline(&self, depth: usize, out: &mut String) {
        // 通用空容器(无角色无名)不占一行,子代提到同层,保持大纲精简。
        if self.is_blank() {
            for c in &self.children {
                c.write_outline(depth, out);
            }
            return;
        }
        for _ in 0..depth {
            out.push_str("  ");
        }
        if self.role.is_empty() {
            out.push_str("text");
        } else {
            out.push_str(&self.role);
        }
        if !self.name.is_empty() {
            out.push_str(" \"");
            out.push_str(&self.name);
            out.push('"');
        }
        if !self.properties.is_empty() {
            let kv: Vec<String> = self
                .properties
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            out.push_str(" [");
            out.push_str(&kv.join(", "));
            out.push(']');
        }
        out.push('\n');
        for c in &self.children {
            c.write_outline(depth + 1, out);
        }
    }
}

/// 一棵无障碍树(根节点 + 检索 / 大纲 / 导出)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxTree {
    /// 根节点。
    pub root: AxNode,
}

impl AxTree {
    /// 包装一个根节点。
    pub fn new(root: AxNode) -> Self {
        Self { root }
    }

    /// 节点总数。
    pub fn count(&self) -> usize {
        self.root.count()
    }

    /// 所有节点引用(前序)。
    pub fn nodes(&self) -> Vec<&AxNode> {
        let mut out = Vec::new();
        self.root.collect(&mut out);
        out
    }

    /// 按**角色**精确检索(如 `"button"`)。
    pub fn find_by_role(&self, role: &str) -> Vec<&AxNode> {
        self.nodes().into_iter().filter(|n| n.role == role).collect()
    }

    /// 按**可见名**子串检索(大小写敏感)。
    pub fn find_by_name(&self, substr: &str) -> Vec<&AxNode> {
        self.nodes()
            .into_iter()
            .filter(|n| n.name.contains(substr))
            .collect()
    }

    /// 缩进文本大纲,每行 `role "name" [props]`(给人读 / 喂 LLM 最省 token)。
    pub fn to_outline(&self) -> String {
        let mut s = String::new();
        self.root.write_outline(0, &mut s);
        s
    }

    /// 序列化为美化 JSON。
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }
}

/// 从 CDP `Accessibility.getFullAXTree` 的结果(含 `nodes` 扁平数组)**重建**成 [`AxTree`]。
///
/// 跳过 `ignored` 节点(把其子代**提升**到原位),按 `childIds` 关联父子;根为无 `parentId` 者
/// (多个根则套一个空容器根)。对缺失/环引用做了防护。
pub fn build_from_cdp(result: &Value) -> AxTree {
    let nodes = result["nodes"].as_array().cloned().unwrap_or_default();

    // 解析每个节点:id → (ignored, 部分 AxNode(无 children), childIds)。
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut parsed: Vec<(bool, AxNode, Vec<String>)> = Vec::with_capacity(nodes.len());
    let mut has_parent: BTreeMap<String, bool> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();

    for n in &nodes {
        let Some(id) = n["nodeId"].as_str() else {
            continue;
        };
        let ignored = n["ignored"].as_bool().unwrap_or(false);
        let role = ax_field(&n["role"]);
        let name = ax_field(&n["name"]);
        let value = non_empty(ax_field(&n["value"]));
        let description = non_empty(ax_field(&n["description"]));
        let properties = parse_cdp_props(&n["properties"]);
        let child_ids: Vec<String> = n["childIds"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        for c in &child_ids {
            has_parent.insert(c.clone(), true);
        }
        index.insert(id.to_string(), parsed.len());
        order.push(id.to_string());
        parsed.push((
            ignored,
            AxNode {
                role,
                name,
                value,
                description,
                properties,
                children: Vec::new(),
            },
            child_ids,
        ));
    }

    // 根:首个无 parentId 的节点(无则取首个)。
    let root_id = order
        .iter()
        .find(|id| !has_parent.get(*id).copied().unwrap_or(false))
        .or_else(|| order.first())
        .cloned();

    let mut visited = std::collections::BTreeSet::new();
    let roots = match root_id {
        Some(id) => assemble_cdp(&id, &index, &parsed, &mut visited),
        None => Vec::new(),
    };

    let root = match roots.len() {
        1 => roots.into_iter().next().unwrap(),
        _ => AxNode {
            role: String::new(),
            name: String::new(),
            children: roots,
            ..Default::default()
        },
    };
    AxTree::new(root)
}

/// 递归装配:返回该 id 处的"可见"节点列表(`ignored` 节点丢自身、提升子代)。
fn assemble_cdp(
    id: &str,
    index: &BTreeMap<String, usize>,
    parsed: &[(bool, AxNode, Vec<String>)],
    visited: &mut std::collections::BTreeSet<String>,
) -> Vec<AxNode> {
    if !visited.insert(id.to_string()) {
        return Vec::new(); // 防环
    }
    let Some(&i) = index.get(id) else {
        return Vec::new();
    };
    let (ignored, node, child_ids) = &parsed[i];
    let mut kids = Vec::new();
    for c in child_ids {
        kids.extend(assemble_cdp(c, index, parsed, visited));
    }
    if *ignored {
        kids
    } else {
        let mut me = node.clone();
        me.children = kids;
        vec![me]
    }
}

/// 取 CDP AX 字段(形如 `{ "value": ... }`)里的字符串值(其它类型 stringify)。
fn ax_field(field: &Value) -> String {
    match &field["value"] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// 解析 CDP AX `properties`:`[{name, value:{value}}]` → 仅保留若干**关键状态**(避免噪声)。
fn parse_cdp_props(props: &Value) -> BTreeMap<String, String> {
    const KEEP: &[&str] = &[
        "checked", "disabled", "expanded", "focused", "selected", "pressed", "required",
        "level", "invalid",
    ];
    let mut out = BTreeMap::new();
    if let Some(arr) = props.as_array() {
        for p in arr {
            let Some(name) = p["name"].as_str() else {
                continue;
            };
            if !KEEP.contains(&name) {
                continue;
            }
            let v = match &p["value"]["value"] {
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                _ => continue,
            };
            // CDP 常用 "false"/"true"/"mixed";"false" 的布尔状态略去更干净。
            if v == "false" {
                continue;
            }
            out.insert(name.to_string(), v);
        }
    }
    out
}

/// 解析 [`AX_SNAPSHOT_JS`] 返回的近似语义树(形如 `{role,name,value?,props?,children?}`)为 [`AxTree`]。
pub fn build_from_snapshot(value: &Value) -> AxTree {
    AxTree::new(parse_snapshot_node(value))
}

fn parse_snapshot_node(v: &Value) -> AxNode {
    let role = v["role"].as_str().unwrap_or("").to_string();
    let name = v["name"].as_str().unwrap_or("").trim().to_string();
    let value = non_empty(v["value"].as_str().unwrap_or("").to_string());
    let mut properties = BTreeMap::new();
    if let Some(obj) = v["props"].as_object() {
        for (k, val) in obj {
            let s = match val {
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                _ => continue,
            };
            properties.insert(k.clone(), s);
        }
    }
    let children = v["children"]
        .as_array()
        .map(|a| a.iter().map(parse_snapshot_node).collect())
        .unwrap_or_default();
    AxNode {
        role,
        name,
        value,
        description: None,
        properties,
        children,
    }
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

/// 注入页面、按 ARIA 规则算**近似**语义树的 JS 表达式(返回 `{role,name,value?,props?,children?}`)。
///
/// 显式 `role` 优先,否则按标签推断隐式角色;可见名取 aria-label/labelledby/alt/placeholder/文本;
/// 跳过不可见与非内容标签;无信息的通用容器被裁剪/提升。供 `run_js`(returnByValue)取回。
pub const AX_SNAPSHOT_JS: &str = r#"(()=>{
  const TAG_ROLE={a:'link',button:'button',nav:'navigation',main:'main',header:'banner',
    footer:'contentinfo',aside:'complementary',h1:'heading',h2:'heading',h3:'heading',
    h4:'heading',h5:'heading',h6:'heading',img:'img',ul:'list',ol:'list',li:'listitem',
    table:'table',thead:'rowgroup',tbody:'rowgroup',tr:'row',td:'cell',th:'columnheader',
    form:'form',select:'combobox',textarea:'textbox',option:'option',section:'region',
    article:'article',dialog:'dialog',fieldset:'group',legend:'',summary:'button'};
  const INPUT_ROLE={checkbox:'checkbox',radio:'radio',button:'button',submit:'button',
    reset:'button',range:'slider',search:'searchbox',email:'textbox',tel:'textbox',
    url:'textbox',number:'spinbutton',password:'textbox',text:'textbox'};
  const SKIP={script:1,style:1,noscript:1,template:1,head:1,meta:1,link:1,br:1,svg:1,path:1};
  const roleOf=(el)=>{
    const ex=el.getAttribute('role'); if(ex) return ex.trim().split(/\s+/)[0];
    const tag=el.tagName.toLowerCase();
    if(tag==='a') return el.hasAttribute('href')?'link':'';
    if(tag==='input'){const t=(el.getAttribute('type')||'text').toLowerCase();return INPUT_ROLE[t]||'textbox';}
    return TAG_ROLE[tag]||'';
  };
  const ownText=(el)=>{let s='';for(const n of el.childNodes){if(n.nodeType===3)s+=n.textContent;}return s.trim();};
  const nameOf=(el)=>{
    const al=el.getAttribute('aria-label'); if(al&&al.trim()) return al.trim();
    const lb=el.getAttribute('aria-labelledby');
    if(lb){const r=lb.split(/\s+/).map(id=>{const n=document.getElementById(id);return n?(n.innerText||''):'';}).join(' ').trim();if(r)return r;}
    const tag=el.tagName.toLowerCase();
    if(tag==='img') return (el.getAttribute('alt')||'').trim();
    if(tag==='input'){const t=(el.getAttribute('type')||'text').toLowerCase();
      if(t==='submit'||t==='button'||t==='reset') return (el.value||'').trim();
      return (el.getAttribute('placeholder')||el.getAttribute('name')||'').trim();}
    const o=ownText(el); if(o) return o.slice(0,120);
    if(['button','a','h1','h2','h3','h4','h5','h6','label','option','summary','td','th','legend'].includes(tag))
      return (el.innerText||'').trim().slice(0,120);
    return '';
  };
  const visible=(el)=>{const s=window.getComputedStyle(el);
    if(!s||s.display==='none'||s.visibility==='hidden'||s.visibility==='collapse')return false;
    if(el.hasAttribute('hidden')||el.getAttribute('aria-hidden')==='true')return false;return true;};
  const propsOf=(el)=>{const p={};
    if(el.hasAttribute('disabled'))p.disabled='true';
    const ac=el.getAttribute('aria-checked');
    if(ac){if(ac!=='false')p.checked=ac;}
    else if(el.tagName==='INPUT'&&(el.type==='checkbox'||el.type==='radio')){if(el.checked)p.checked='true';}
    const ae=el.getAttribute('aria-expanded'); if(ae)p.expanded=ae;
    const as=el.getAttribute('aria-selected'); if(as&&as!=='false')p.selected=as;
    if(/^h[1-6]$/.test(el.tagName.toLowerCase()))p.level=el.tagName[1];
    return p;};
  const valueOf=(el)=>{const tag=el.tagName.toLowerCase();
    if(tag==='input'){const t=(el.getAttribute('type')||'text').toLowerCase();
      if(['text','email','tel','url','number','password','search'].includes(t))return el.value||'';}
    if(tag==='textarea')return el.value||'';return '';};
  let count=0;const MAX=4000;
  function build(el){
    if(count>MAX||!el||el.nodeType!==1)return null;
    if(SKIP[el.tagName.toLowerCase()])return null;
    if(!visible(el))return null;
    count++;
    const kids=[];for(const c of el.children){const k=build(c);if(k)kids.push(k);}
    const role=roleOf(el),name=nameOf(el),pr=propsOf(el),val=valueOf(el);
    if(!role&&!name&&!val&&Object.keys(pr).length===0){
      if(kids.length===0)return null;
      if(kids.length===1)return kids[0];
      return {role:'',name:'',children:kids};
    }
    const node={role:role,name:name};
    if(val)node.value=val;
    if(Object.keys(pr).length)node.props=pr;
    if(kids.length)node.children=kids;
    return node;
  }
  return build(document.body)||{role:'',name:''};
})()"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn n(role: &str, name: &str) -> AxNode {
        AxNode {
            role: role.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn cdp_flat_rebuilds_tree_and_skips_ignored() {
        // 1(root) → [2(ignored) , 3(button)] ; 2 → [4(StaticText)]。
        // 4 应被提升到 root 下(因为 2 被跳过)。
        let r = json!({"nodes":[
            {"nodeId":"1","role":{"value":"RootWebArea"},"name":{"value":"Doc"},"childIds":["2","3"]},
            {"nodeId":"2","ignored":true,"role":{"value":"generic"},"name":{"value":""},
             "parentId":"1","childIds":["4"]},
            {"nodeId":"3","role":{"value":"button"},"name":{"value":"OK"},"parentId":"1","childIds":[]},
            {"nodeId":"4","role":{"value":"StaticText"},"name":{"value":"hi"},"parentId":"2","childIds":[]}
        ]});
        let t = build_from_cdp(&r);
        assert_eq!(t.root.role, "RootWebArea");
        assert_eq!(t.root.children.len(), 2); // StaticText(提升) + button
        let roles: Vec<&str> = t.root.children.iter().map(|c| c.role.as_str()).collect();
        assert!(roles.contains(&"StaticText") && roles.contains(&"button"));
        assert_eq!(t.find_by_role("button").len(), 1);
        assert_eq!(t.find_by_role("button")[0].name, "OK");
    }

    #[test]
    fn cdp_props_keep_only_truthy_state() {
        let r = json!({"nodes":[
            {"nodeId":"1","role":{"value":"checkbox"},"name":{"value":"Agree"},"childIds":[],
             "properties":[
                {"name":"checked","value":{"value":"true"}},
                {"name":"disabled","value":{"value":"false"}},
                {"name":"live","value":{"value":"polite"}}
             ]}
        ]});
        let t = build_from_cdp(&r);
        // checked=true 保留;disabled=false 略去;live 不在白名单。
        assert_eq!(t.root.properties.get("checked").map(String::as_str), Some("true"));
        assert!(!t.root.properties.contains_key("disabled"));
        assert!(!t.root.properties.contains_key("live"));
    }

    #[test]
    fn outline_indents_and_collapses_blank() {
        let mut root = n("", ""); // 空容器根
        let mut form = n("form", "Login");
        form.children.push(n("textbox", "User"));
        let mut btn = n("button", "Submit");
        btn.properties.insert("disabled".into(), "true".into());
        form.children.push(btn);
        root.children.push(form);
        let t = AxTree::new(root);
        let o = t.to_outline();
        // 空根不占行,form 在第 0 层、子在第 1 层。
        assert!(o.starts_with("form \"Login\"\n"));
        assert!(o.contains("\n  textbox \"User\"\n"));
        assert!(o.contains("\n  button \"Submit\" [disabled=true]\n"));
    }

    #[test]
    fn find_helpers_and_count() {
        let mut root = n("RootWebArea", "");
        root.children.push(n("link", "Home"));
        root.children.push(n("link", "About us"));
        root.children.push(n("button", "Buy"));
        let t = AxTree::new(root);
        assert_eq!(t.count(), 4);
        assert_eq!(t.find_by_role("link").len(), 2);
        assert_eq!(t.find_by_name("us").len(), 1);
        assert_eq!(t.find_by_name("us")[0].role, "link");
    }

    #[test]
    fn snapshot_json_parsed() {
        let v = json!({"role":"form","name":"Login","children":[
            {"role":"textbox","name":"User","value":"bob","props":{"disabled":"true"}},
            {"role":"button","name":"Go"}
        ]});
        let t = build_from_snapshot(&v);
        assert_eq!(t.root.role, "form");
        assert_eq!(t.root.children.len(), 2);
        let tb = &t.root.children[0];
        assert_eq!(tb.value.as_deref(), Some("bob"));
        assert_eq!(tb.properties.get("disabled").map(String::as_str), Some("true"));
    }

    #[test]
    fn to_json_skips_empty_fields() {
        let t = AxTree::new(n("button", "OK"));
        let j = t.to_json();
        assert!(j.contains("\"role\": \"button\""));
        // 空 value/description/properties/children 不应出现。
        assert!(!j.contains("\"value\""));
        assert!(!j.contains("\"children\""));
    }
}
