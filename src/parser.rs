use std::cell::RefCell;
use std::error::Error;
use std::fmt::Debug;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::rc::Rc;
use std::{fmt, io};

use itertools::Itertools;

use crate::lexer::{Lexer, Token, TokenKind};

pub(crate) type SharedLine = Rc<RefCell<Line>>;

pub(crate) trait HtmlGenerate {
    fn gen_title(&self, l: &SharedLine) -> String;
    fn gen_dividling(&self, l: &SharedLine) -> String;
    fn gen_normal(&self, ls: &[SharedLine]) -> String;
    fn gen_blank(&self, ls: &[SharedLine]) -> String;
    fn gen_sorted_list(&self, ls: &[SharedLine]) -> String;
    fn gen_disordered_list(&self, ls: &[SharedLine]) -> String;
    fn gen_quote(&self, ls: &[SharedLine]) -> String;
    fn gen_code(&self, ls: &[SharedLine]) -> String;
}

// Ast represents the abstract syntax tree of the markdown file, it structurally represents the entire file.
pub(crate) struct Ast {
    document: Vec<SharedLine>,
    blocks: Vec<Block>,
    defer_queue: Vec<SharedLine>,
    enabled_defer: bool,
}

impl Ast {
    // Create a Ast instance.
    pub(crate) fn new() -> Self {
        Ast {
            document: vec![Rc::new(RefCell::new(Line::meta()))],
            blocks: vec![],
            defer_queue: vec![],
            enabled_defer: false,
        }
    }

    // Parse markdown document from a file, the 'path' argument is the file path.
    pub(crate) fn parse_file(&mut self, path: &str) -> Result<(), io::Error> {
        let file = File::open(path)?;
        self.parse_from(&mut BufReader::new(file))
    }

    // Parse markdown document from a string.
    pub(crate) fn parse_string(&mut self, s: &str) -> Result<(), io::Error> {
        self.parse_from(&mut s.as_bytes())
    }

    // Parse markdown document from a reader, the 'reader' may be a file reader, byte buff or network socket etc.
    pub(crate) fn parse_from(&mut self, reader: &mut dyn BufRead) -> Result<(), io::Error> {
        let mut num: usize = 0;
        loop {
            let mut buf = String::new();
            let num_bytes = reader.read_line(&mut buf)?;
            if num_bytes == 0 {
                break;
            }
            if !buf.ends_with('\n') {
                buf.push('\n');
            }
            num += 1;
            self.parse_line(num, buf);
        }
        self.defer_queue.iter().for_each(|l| l.borrow_mut().parse());
        self.defer_queue.clear();
        self.blocks = Ast::establish_blocks(&self.document);
        Ok(())
    }

    // Iterate through each block of the Ast and process the block into a 'html' string
    pub(crate) fn render_html(&self, html: &impl HtmlGenerate) -> Result<String, Box<dyn Error>> {
        let ss: Vec<String> = vec![
            String::from(
                r#"<!doctype html>
<html>
<head>
<meta charset='UTF-8'><meta name='viewport' content='width=device-width initial-scale=1'>
<title></title>
</head>


<body>"#,
            ),
            String::from("</body></html>"),
        ];

        let gens = self
            .blocks()
            .iter()
            .filter(|b| b.kind() != Kind::Meta && b.kind() != Kind::ListNesting)
            .map(|b| match b.kind() {
                Kind::Title => html.gen_title(b.first()),
                Kind::NormalText => html.gen_normal(b.lines()),
                Kind::DividingLine => html.gen_dividling(b.first()),
                Kind::Code => html.gen_code(b.lines()),
                Kind::DisorderedList => html.gen_disordered_list(b.lines()),
                Kind::Blank => html.gen_blank(b.lines()),
                Kind::Quote => html.gen_quote(b.lines()),
                Kind::SortedList => html.gen_sorted_list(b.lines()),
                Kind::CodeMark => html.gen_normal(b.lines()), // as normal text
                _ => unreachable!(),
            })
            .filter(|s| !s.is_empty())
            .join("\n\n");

        Ok(Itertools::intersperse(ss.iter(), &gens).join("\n"))
    }

    // Count the lines in ast
    pub(crate) fn count_lines(&self) -> usize {
        self.document.len() - 1
    }

    // Get the Line object with line number
    pub(crate) fn _get_line(&self, num: usize) -> Option<&SharedLine> {
        self.document.get(num)
    }

    fn blocks(&self) -> &Vec<Block> {
        &self.blocks
    }

    fn parse_line(&mut self, num: usize, line: String) {
        let l = if self.defer_parse(&line) {
            let c = RefCell::new(Line::new_without_parsing(num, line));
            let l1 = Rc::new(c);
            let l2 = Rc::clone(&l1);
            self.defer_queue.push(l1);
            l2
        } else {
            let c = RefCell::new(Line::new(num, line));
            Rc::new(c)
        };
        if l.borrow().kind == Kind::CodeMark {
            self.enabled_defer = !self.enabled_defer;
            if !self.enabled_defer {
                // closed code block
                self.defer_queue.clear();
            }
        }
        self.document.push(l);

        debug_assert_eq!(self.count_lines(), num);
        debug_assert_eq!(self.document[num].borrow().num, num);
    }

    fn defer_parse(&self, s: &str) -> bool {
        self.enabled_defer && s.trim() != "```"
    }

    fn establish_blocks(all: &[SharedLine]) -> Vec<Block> {
        let mut blocks: Vec<Block> = vec![];

        let mut leader: Option<&SharedLine> = None;
        let mut state: Option<Kind> = None;

        let mut pre_line_kind: Option<Kind> = None;

        let mut iter = all
            .iter()
            .filter(|x| x.borrow().kind != Kind::Meta)
            .peekable();
        while let Some(l) = iter.next() {
            let mut curr_line = l.borrow_mut();
            // Note: Don't use 'continue' statement in the match expression, because
            // we will save the kind of the previous block after the 'match'.
            match state.unwrap_or(curr_line.kind) {
                Kind::DisorderedList | Kind::SortedList => {
                    if let Some(b) = blocks.last_mut().filter(|b| b.kind() == curr_line.kind) {
                        b.push(Rc::clone(l));
                    } else {
                        Ast::insert_block(&mut blocks, Block::new(Rc::clone(l), curr_line.kind));
                    }

                    // Check the next line is a list nesting or not
                    if let Some(next) = iter.peek() {
                        let next = next.borrow();
                        if next.indents() - curr_line.indents() == 1 // indent 1
                            && next.kind != Kind::Blank
                            && next.kind != Kind::Title
                            && next.kind != Kind::DividingLine
                            && next.kind != Kind::CodeMark
                            && next.kind != Kind::Code
                        {
                            state = Some(Kind::ListNesting);
                            leader = Some(l); // save the previous line object as leader
                        }
                    }
                }
                Kind::ListNesting => {
                    // leader must not be None
                    debug_assert!(leader.is_some());

                    if let Some(ld) = leader {
                        let mut ld = ld.borrow_mut();
                        ld.nested_lines.push(Rc::clone(l));

                        if let Some(next) = iter.peek() {
                            let next = next.borrow();
                            if next.indents() - ld.indents() >= 1 // at least indent 1
                            && next.kind != Kind::Blank
                            && next.kind != Kind::Title
                            && next.kind != Kind::DividingLine
                            && next.kind != Kind::CodeMark
                            && next.kind != Kind::Code
                            {
                                // keep the state
                            } else {
                                state = None;
                                leader = None;
                            }
                        }
                    }
                }

                Kind::CodeMark => {
                    let mut k: Option<Kind> = None;
                    if let Some(next) = iter.peek() {
                        let next = next.borrow();
                        if next.kind == Kind::CodeMark || next.kind == Kind::Code {
                            k = Some(Kind::Code);
                            state = Some(Kind::Code);
                        }
                    }
                    Ast::insert_block(
                        &mut blocks,
                        Block::new(Rc::clone(l), k.unwrap_or(Kind::CodeMark)),
                    );
                }
                Kind::Code => {
                    let b = blocks.last_mut().filter(|b| b.kind() == Kind::Code);
                    debug_assert!(b.is_some());

                    if let Some(b) = b {
                        b.push(Rc::clone(l));
                    }

                    if curr_line.kind == Kind::CodeMark {
                        // close this code block
                        state = None;
                    }
                }

                Kind::Blank | Kind::Quote | Kind::NormalText => {
                    if let Some(b) = blocks.last_mut().filter(|b| b.kind() == curr_line.kind) {
                        b.push(Rc::clone(l));
                    } else {
                        Ast::insert_block(&mut blocks, Block::new(Rc::clone(l), curr_line.kind));
                    }
                }

                Kind::DividingLine => {
                    let next_line_kind =
                        iter.peek().map(|l| l.borrow().kind).unwrap_or(Kind::Blank);

                    if pre_line_kind.unwrap_or(Kind::Blank) == Kind::Blank
                        && next_line_kind == Kind::Blank
                    {
                        Ast::insert_block(&mut blocks, Block::new(Rc::clone(l), curr_line.kind))
                    } else {
                        curr_line.kind = Kind::NormalText;
                        curr_line
                            .buff
                            .iter_mut()
                            .for_each(|t| t.update_kind(TokenKind::Text));

                        if let Some(b) = blocks.last_mut().filter(|b| b.kind() == Kind::NormalText)
                        {
                            b.push(Rc::clone(l));
                        } else {
                            Ast::insert_block(
                                &mut blocks,
                                Block::new(Rc::clone(l), curr_line.kind),
                            );
                        }
                    }
                }

                Kind::Title => {
                    Ast::insert_block(&mut blocks, Block::new(Rc::clone(l), curr_line.kind));
                }
                Kind::Meta => unreachable!(),
            }

            // save the kind of previous block
            pre_line_kind = Some(curr_line.kind);
        }

        // build nested lists recursively
        for b in blocks
            .iter()
            .filter(|b| b.kind() == Kind::DisorderedList || b.kind() == Kind::SortedList)
        {
            b.lines()
                .iter()
                .filter(|l| !l.borrow().nested_lines.is_empty())
                .for_each(|l| {
                    let mut l = l.borrow_mut();
                    let mut bs = Ast::establish_blocks(&l.nested_lines);
                    l.nested_blocks.append(&mut bs);
                });
        }

        blocks
    }

    fn insert_block(blocks: &mut Vec<Block>, mut b: Block) {
        b.seq = blocks.len();
        blocks.push(b);
    }
}

impl Default for Ast {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for Ast {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = String::new();
        for line in self.document.iter() {
            debug.push_str(format!("[{}, {:?}]: ", line.borrow().num, line.borrow().kind).as_str());
            for t in line.borrow().all() {
                let s = format!("{:?} ", t);
                debug.push_str(&s);
            }
            debug.push('\n');
        }
        writeln!(f, "{}", debug)
    }
}

// Block is a group of multiple lines.
#[derive(Debug)]
struct Block {
    indices: Vec<SharedLine>,
    kind: Kind,
    seq: usize,
    // pre: Weak<RefCell<Block>>,
    // next: Rc<RefCell<Block>>,
}

impl Block {
    fn new(l: SharedLine, kind: Kind) -> Self {
        Block {
            indices: vec![l],
            kind,
            seq: 0,
        }
    }

    fn lines(&self) -> &Vec<SharedLine> {
        &self.indices
    }

    fn first(&self) -> &SharedLine {
        &self.indices[0]
    }

    fn kind(&self) -> Kind {
        self.kind
    }

    fn push(&mut self, l: SharedLine) {
        self.indices.push(l)
    }

    fn _count(&self) -> usize {
        self.indices.len()
    }
}

#[derive(Debug, PartialEq, Copy, Clone)]
enum Kind {
    NormalText,
    Blank,
    Title,
    DisorderedList,
    SortedList,
    DividingLine,
    Quote,
    CodeMark,
    Code,
    Meta,
    ListNesting,
}

// Line is a line of the markdown file, it be parsed into some tokens.
#[derive(Debug)]
pub(crate) struct Line {
    buff: Vec<Token>,
    kind: Kind,
    num: usize,
    text: String,
    //
    // The lines in nesting:
    //   Kind::DisorderedList
    //   Kind::SortedList
    //   Kind::Quote
    //   Kind::Normal
    nested_lines: Vec<SharedLine>,
    nested_blocks: Vec<Block>,
}

impl Line {
    fn new(ln: usize, line: String) -> Self {
        let mut l = Line {
            num: ln,
            text: line,
            kind: Kind::NormalText,
            buff: vec![],
            nested_lines: vec![],
            nested_blocks: vec![],
        };
        l.parse();
        l
    }

    fn new_without_parsing(ln: usize, line: String) -> Self {
        Line {
            num: ln,
            text: line,
            kind: Kind::Code,
            buff: vec![],
            nested_lines: vec![],
            nested_blocks: vec![],
        }
    }

    fn meta() -> Self {
        Line {
            buff: vec![],
            kind: Kind::Meta,
            num: 0,
            text: "meta".to_string(),
            nested_lines: vec![],
            nested_blocks: vec![],
        }
    }

    pub(crate) fn enter_nested_blocks(&self, html: &impl HtmlGenerate) -> String {
        self.nested_blocks
            .iter()
            .filter(|b| {
                b.kind() != Kind::Meta
                    && b.kind() != Kind::ListNesting
                    && b.kind() != Kind::Blank
                    && b.kind() != Kind::Title
                    && b.kind() != Kind::DividingLine
                    && b.kind() != Kind::CodeMark
                    && b.kind() != Kind::Code
            })
            .map(|b| match b.kind() {
                Kind::Title => html.gen_title(b.first()),
                Kind::NormalText => html.gen_normal(b.lines()),
                Kind::DividingLine => html.gen_dividling(b.first()),
                Kind::Code => html.gen_code(b.lines()),
                Kind::DisorderedList => html.gen_disordered_list(b.lines()),
                Kind::Blank => html.gen_blank(b.lines()),
                Kind::Quote => html.gen_quote(b.lines()),
                Kind::SortedList => html.gen_sorted_list(b.lines()),
                _ => "".to_string(),
            })
            .join("\n")
    }

    // Get the mark token in the Line, the mark token may be the first or second
    pub(crate) fn get_mark(&self) -> &Token {
        let first = self.first_token();
        if first.kind() == TokenKind::WhiteSpace {
            // if the first token is 'WhiteSpace', the second token must be exist
            &self.buff[1]
        } else {
            first
        }
    }

    // Get the Nth Token in the Line
    pub(crate) fn get(&self, at: usize) -> Option<&Token> {
        self.buff.get(at)
    }

    // Get all tokens in the Line
    pub(crate) fn all(&self) -> &Vec<Token> {
        &self.buff
    }

    // Get the line text
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    // Parses a line of text into 'Line' struct that contains multi tokens.
    // Line's kind is determinded by the first token's kind.
    fn parse(&mut self) {
        self.buff = Lexer::new(&self.text).split();

        self.kind = match self.get_mark().kind() {
            TokenKind::BlankLine => Kind::Blank,
            TokenKind::TitleMark => Kind::Title,
            TokenKind::DisorderListMark => Kind::DisorderedList,
            TokenKind::SortedListMark => Kind::SortedList,
            TokenKind::DividingMark => Kind::DividingLine,
            TokenKind::QuoteMark => Kind::Quote,
            TokenKind::CodeMark => Kind::CodeMark,
            _ => Kind::NormalText,
        };

        debug_assert!(!self.all().is_empty());
    }

    // Get number of the indent, two white space(' ') or one '\t' is a indent
    fn indents(&self) -> usize {
        let first = self.first_token();
        if first.kind() != TokenKind::WhiteSpace {
            return 0;
        }
        let sum: usize = first
            .value()
            .chars()
            .map(|c| if c == '\t' { 2 } else { 1 })
            .sum();
        sum / 2
    }

    fn first_token(&self) -> &Token {
        &self.buff[0]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn create_ast(s: &str) -> Ast {
        let mut ast = Ast::new();
        ast.parse_string(s).unwrap();
        ast
    }

    #[test]
    fn test_parse_all() {
        let md = "# dice 是一个 markdown 编辑器
这是我的一个学习 rust 编程语言的项目，我将尝试去开发一个强大的 markdown 编辑器。

重点:
* 支持 vim 操作
* 支持自动化的命令去编辑文档
* 扩展内容的左右水平布局
* 更好交互的表格
* 支持幻灯片模式的播放
* 必须是快速的
* ...

1. 支持 vim 操作
2. 支持自动化的命令去编辑文档
3. 扩展内容的左右水平布局
4. 更好交互的表格
5. 支持幻灯片模式的播放
6. 必须是快速的
7. ...

---
****
_____________
--- 这不是一个分界线
---a 这也不是一个分界线

这里带有换行  
新的一行

![这是图片](/assets/img/philly-magic-garden.jpg \"Magic Gardens\")
这里是一个链接 [链接](https://a.com/assets/img/philly-magic-garden.jpg \"Magic Gardens\")

> Rust, A language empowering everyone to build reliable and efficient software.";

        let mut ast = Ast::new();
        ast.parse_string(md).unwrap();
        assert_eq!(ast.count_lines(), md.lines().count());
    }

    #[test]
    fn test_establish_blocks() {
        let md = "# dice 是一个 markdown 编辑器
这是我的一个学习 rust 编程语言的项目，我将尝试去开发一个强大的 markdown 编辑器。

* 支持 vim 操作
  vim1
* 支持自动化的命令去编辑文档

1. 支持 vim 操作
2. 支持自动化的命令去编辑文档
3. 扩展内容的左右水平布局


> Rust, A language empowering everyone to build reliable and efficient software.";

        let mut ast = Ast::new();
        ast.parse_string(md).unwrap();

        assert_eq!(ast.count_lines(), md.lines().count());
        assert_eq!(ast.blocks.len(), 8);

        assert_eq!(
            ast.blocks[3]
                .lines()
                .get(0)
                .unwrap()
                .borrow()
                .nested_lines
                .len(),
            1
        );
        assert_eq!(
            ast.blocks[3]
                .lines()
                .get(0)
                .unwrap()
                .borrow()
                .nested_blocks
                .len(),
            1
        );
    }

    #[test]
    fn test_parse_title() {
        let marks = vec!["# ", "## ", "### ", "#### "];
        let titles = vec!["Header1", "Header1 ", " Header1 Header1", " ", ""];

        for mark in marks {
            let mut n = 0;
            let mut ast = Ast::new();
            for title in titles.iter() {
                let mut s = mark.to_string();
                s += title;
                s += "\n";

                n += 1;
                ast.parse_line(n, s)
            }

            assert_eq!(ast.count_lines(), n as usize);
            {
                let l = ast._get_line(1).unwrap().borrow();
                assert_eq!(l.kind, Kind::Title);
                assert_eq!(
                    l.buff,
                    vec![
                        Token::new(mark.trim_end().to_string(), TokenKind::TitleMark),
                        Token::new(titles.get(0).unwrap().trim().to_string(), TokenKind::Text,)
                    ]
                );
            }
            {
                let l = ast._get_line(2).unwrap().borrow();
                assert_eq!(l.kind, Kind::Title);
                assert_eq!(
                    l.buff,
                    vec![
                        Token::new(mark.trim_end().to_string(), TokenKind::TitleMark,),
                        Token::new(titles.get(1).unwrap().trim().to_string(), TokenKind::Text,)
                    ]
                );
            }
            {
                let l = ast._get_line(3).unwrap().borrow();
                assert_eq!(l.kind, Kind::Title);
                assert_eq!(
                    l.buff,
                    vec![
                        Token::new(mark.trim_end().to_string(), TokenKind::TitleMark,),
                        Token::new(titles.get(2).unwrap().to_string(), TokenKind::Text,)
                    ]
                );
            }
            {
                let l = ast._get_line(4).unwrap().borrow();
                assert_eq!(l.kind, Kind::Title);
                assert_eq!(
                    l.buff,
                    vec![Token::new(
                        mark.trim_end().to_string(),
                        TokenKind::TitleMark
                    )]
                );
            }
            {
                let l = ast._get_line(5).unwrap().borrow();
                assert_eq!(l.kind, Kind::Title);
                assert_eq!(
                    l.buff,
                    vec![Token::new(
                        mark.trim_end().to_string(),
                        TokenKind::TitleMark
                    )]
                );
            }
        }
    }

    #[test]
    fn test_parse_title2() {
        let title = "# header1 [这是一个链接](https://example.com)";
        let ast = create_ast(title);

        assert_eq!(ast.count_lines(), 1);
        assert_eq!(ast._get_line(1).unwrap().borrow().kind, Kind::Title);

        let mut kvs = HashMap::new();
        kvs.insert("show_name".to_string(), "这是一个链接".to_string());
        kvs.insert("location".to_string(), "https://example.com".to_string());

        let mut url = Token::new(
            "[这是一个链接](https://example.com)".to_string(),
            TokenKind::Url,
        );
        url.details = Some(kvs);
        assert_eq!(
            ast._get_line(1).unwrap().borrow().buff,
            vec![
                Token::new("#".to_string(), TokenKind::TitleMark),
                Token::new("header1 ".to_string(), TokenKind::Text),
                url,
            ]
        );
    }

    #[test]
    fn test_parse_dividing() {
        let marks = vec![
            "---",
            "***",
            "___",
            "------",
            "****",
            "__________         ",
            "----------------------------------------   ",
        ];

        for mark in marks {
            let ast = create_ast(mark);

            assert_eq!(ast.count_lines(), 1);
            assert_eq!(
                ast.document
                    .iter()
                    .skip(1)
                    .map(|x| x.borrow().kind)
                    .collect::<Vec<Kind>>(),
                vec![Kind::DividingLine],
            );
            assert_eq!(
                ast._get_line(1).unwrap().borrow().buff,
                vec![Token::new(
                    mark.trim_end().to_string(),
                    TokenKind::DividingMark,
                )]
            );
        }
    }

    #[test]
    fn test_parse_normal() {
        {
            let contents = vec![
                "这是我的一个学习 rust 编程语言的项目，我将尝试去开发一个强大的 markdown 编辑器。",
                "--- x",
                "___ 这不是一个分界线",
                "--- ---",
                "#这不是标题",
                "##这也不是标题",
                ">这不是引用",
                "1.这也不是列表",
                "***xxxx",
                "**1**",
                "*1*",
                "*** 1 ***",
                "__1__",
                "_1_",
                "___ 1 ___",
            ];

            let expects: Vec<Vec<Token>> = vec![
                vec![Token::new(contents[0].to_string(), TokenKind::Text)],
                vec![Token::new(contents[1].to_string(), TokenKind::Text)],
                vec![
                    // 2
                    Token::new("___".to_string(), TokenKind::Text),
                    Token::new(" 这不是一个分界线".to_string(), TokenKind::Text),
                ],
                vec![Token::new(contents[3].to_string(), TokenKind::Text)],
                vec![Token::new(contents[4].to_string(), TokenKind::Text)],
                vec![Token::new(contents[5].to_string(), TokenKind::Text)],
                vec![Token::new(contents[6].to_string(), TokenKind::Text)],
                vec![Token::new(contents[7].to_string(), TokenKind::Text)],
                vec![
                    // 8
                    Token::new("***".to_string(), TokenKind::Text),
                    Token::new("xxxx".to_string(), TokenKind::Text),
                ],
                vec![
                    // 9
                    Token::new("**".to_string(), TokenKind::BoldMark),
                    Token::new("1".to_string(), TokenKind::Text),
                    Token::new("**".to_string(), TokenKind::BoldMark),
                ],
                vec![
                    // 10
                    Token::new("*".to_string(), TokenKind::ItalicMark),
                    Token::new("1".to_string(), TokenKind::Text),
                    Token::new("*".to_string(), TokenKind::ItalicMark),
                ],
                vec![
                    // 11
                    Token::new("***".to_string(), TokenKind::ItalicBoldMark),
                    Token::new(" 1 ".to_string(), TokenKind::Text),
                    Token::new("***".to_string(), TokenKind::ItalicBoldMark),
                ],
                vec![
                    // 12
                    Token::new("__".to_string(), TokenKind::BoldMark),
                    Token::new("1".to_string(), TokenKind::Text),
                    Token::new("__".to_string(), TokenKind::BoldMark),
                ],
                vec![
                    // 13
                    Token::new("_".to_string(), TokenKind::ItalicMark),
                    Token::new("1".to_string(), TokenKind::Text),
                    Token::new("_".to_string(), TokenKind::ItalicMark),
                ],
                vec![
                    // 14
                    Token::new("___".to_string(), TokenKind::ItalicBoldMark),
                    Token::new(" 1 ".to_string(), TokenKind::Text),
                    Token::new("___".to_string(), TokenKind::ItalicBoldMark),
                ],
            ];

            for (i, cnt) in contents.iter().enumerate() {
                let ast = create_ast(cnt);

                assert_eq!(ast.count_lines(), 1);
                assert_eq!(ast._get_line(1).unwrap().borrow().kind, Kind::NormalText);
                assert_eq!(ast._get_line(1).unwrap().borrow().buff, expects[i]);
            }
        }
    }

    #[test]
    fn test_parse_linebreak() {
        let contents = vec![
            "这是我的一个学习 rust 编程语言的项目，我将尝试去开发一个强大的 markdown 编辑器。  ",  // have two spaces at the end of the line.
            "这是我的一个学习 rust 编程语言的项目，我将尝试去开发一个强大的 markdown 编辑器。       ", // have two tab spaces at the end of the line.
            "这是我的一个学习 rust 编程语言的项目，我将尝试去开发一个强大的 markdown 编辑器。    <br>  ", // have two spaces at the end of the line.
            "这是我的一个学习 rust 编程语言的项目，我将尝试去开发一个强大的 markdown 编辑器。<br>",
        ];

        for cnt in contents {
            let ast = create_ast(cnt);

            assert_eq!(ast.count_lines(), 1);
            assert_eq!(ast._get_line(1).unwrap().borrow().kind, Kind::NormalText);
            assert_eq!(
                ast._get_line(1).unwrap().borrow().buff,
                vec![
                    Token::new(
                        cnt.trim().trim_end_matches("<br>").to_string(),
                        TokenKind::Text
                    ),
                    Token::new("<br>".to_string(), TokenKind::LineBreak)
                ]
            );
        }
    }

    #[test]
    fn test_parse_quote() {
        let mark = ">";
        let content =
            "Rust, A language empowering everyone to build reliable and efficient software.";

        let mut s = mark.to_string();
        s.push_str(" ");
        s.push_str(content);

        let ast = create_ast(&s);

        assert_eq!(ast.count_lines(), 1);
        assert_eq!(ast._get_line(1).unwrap().borrow().kind, Kind::Quote);
        assert_eq!(
            ast._get_line(1).unwrap().borrow().buff,
            vec![
                Token::new(mark.to_string(), TokenKind::QuoteMark),
                Token::new(content.to_string(), TokenKind::Text)
            ]
        );
    }

    #[test]
    fn test_parse_disorder_list() {
        let mark = "*";
        let content =
            "Rust, A language empowering everyone to build reliable and efficient software.";

        let mut s = mark.to_string();
        s.push_str(" ");
        s.push_str(content);

        let ast = create_ast(&s);

        assert_eq!(ast.count_lines(), 1);
        assert_eq!(
            ast._get_line(1).unwrap().borrow().kind,
            Kind::DisorderedList
        );
        assert_eq!(
            ast._get_line(1).unwrap().borrow().buff,
            vec![
                Token::new(mark.to_string(), TokenKind::DisorderListMark),
                Token::new(content.to_string(), TokenKind::Text,)
            ]
        );
    }

    #[test]
    fn test_parse_sorted_list() {
        let marks = vec!["1.", "2.", "3.", "4.", "10.", "11.", "99.", "100.", "999."];
        let content =
            "Rust, A language empowering everyone to build reliable and efficient software.";

        for mark in marks {
            let mut s = mark.to_string();
            s.push_str(" ");
            s.push_str(content);
            let ast = create_ast(&s);

            assert_eq!(ast.count_lines(), 1);
            assert_eq!(ast._get_line(1).unwrap().borrow().kind, Kind::SortedList);
            assert_eq!(
                ast._get_line(1).unwrap().borrow().buff,
                vec![
                    Token::new(mark.to_string(), TokenKind::SortedListMark),
                    Token::new(content.to_string(), TokenKind::Text,)
                ]
            );
        }
    }

    #[test]
    fn test_blank_line() {
        let contents = vec![
            "\n",
            " \n",
            "     \n",
            "         ",
            "                                            ",
            "  ",
        ];

        for cnt in contents {
            let ast = create_ast(cnt);

            assert_eq!(ast.count_lines(), 1);
            assert_eq!(ast._get_line(1).unwrap().borrow().kind, Kind::Blank);
            assert_eq!(
                ast._get_line(1).unwrap().borrow().buff,
                vec![Token::new(
                    cnt.trim_end_matches('\n').to_string(),
                    TokenKind::BlankLine
                )]
            );
        }
    }

    #[test]
    fn test_picture() {
        let pics = vec![
            "![这是图片](/assets/img/philly-magic-garden.jpg \"Magic Gardens\")",
            "![](/assets/img/philly-magic-garden.jpg \"Magic Gardens\")",
            "![]()",
            "![[[[[[]()",
            "![[]]()",
            "![!]()",
            "![![]]()",
            // "![![]()]()",
        ];

        for pic in pics {
            let contents = vec!["text", "text ", "text !", "text !!!!", ""];
            for cnt in contents {
                let s = cnt.to_string() + pic;
                let ast = create_ast(&s);

                assert_eq!(ast.count_lines(), 1);
                assert_eq!(ast._get_line(1).unwrap().borrow().kind, Kind::NormalText);
                if cnt.is_empty() {
                    // token 0
                    assert_eq!(
                        ast._get_line(1).unwrap().borrow().buff[0].kind(),
                        TokenKind::Image
                    );
                    assert_eq!(
                        ast._get_line(1).unwrap().borrow().buff[0].value(),
                        pic.to_string()
                    );
                } else {
                    // token 0
                    assert_eq!(
                        ast._get_line(1).unwrap().borrow().buff[0].kind(),
                        TokenKind::Text
                    );
                    assert_eq!(
                        ast._get_line(1).unwrap().borrow().buff[0].value(),
                        cnt.to_string()
                    );

                    // token 1
                    assert_eq!(
                        ast._get_line(1).unwrap().borrow().buff[1].kind(),
                        TokenKind::Image
                    );
                    assert_eq!(
                        ast._get_line(1).unwrap().borrow().buff[1].value(),
                        pic.to_string()
                    );
                }
            }
        }
    }

    #[test]
    fn test_url() {
        let urls = vec![
            "[这是链接](/assets/img/philly-magic-garden.jpg \"Magic Gardens\")",
            "[](/assets/img/philly-magic-garden.jpg \"Magic Gardens\")",
            "[]()",
            "[]]()",
            "[]]]]()",
            "[!]]()",
        ];

        for url in urls {
            let contents = vec!["text", "text ", "text [", "text [[[[", ""];
            for cnt in contents {
                let s = cnt.to_string() + url;
                let ast = create_ast(&s);

                assert_eq!(ast.count_lines(), 1);
                assert_eq!(ast._get_line(1).unwrap().borrow().kind, Kind::NormalText);
                if cnt.is_empty() {
                    // token 0
                    assert_eq!(
                        ast._get_line(1).unwrap().borrow().buff[0].kind(),
                        TokenKind::Url
                    );
                    assert_eq!(
                        ast._get_line(1).unwrap().borrow().buff[0].value(),
                        url.to_string()
                    );
                } else {
                    // token 0
                    assert_eq!(
                        ast._get_line(1).unwrap().borrow().buff[0].kind(),
                        TokenKind::Text
                    );
                    assert_eq!(
                        ast._get_line(1).unwrap().borrow().buff[0].value(),
                        cnt.to_string()
                    );

                    // token 1
                    assert_eq!(
                        ast._get_line(1).unwrap().borrow().buff[1].kind(),
                        TokenKind::Url
                    );
                    assert_eq!(
                        ast._get_line(1).unwrap().borrow().buff[1].value(),
                        url.to_string()
                    );
                }
            }
        }
    }

    #[test]
    fn test_code_mark() {
        let mut ast = Ast::new();
        ast.parse_string("``` ").unwrap();

        assert_eq!(
            ast.document
                .iter()
                .skip(1)
                .map(|l| l.borrow().kind)
                .collect::<Vec<Kind>>(),
            vec![Kind::CodeMark]
        );

        let mut ast = Ast::new();
        ast.parse_string("```rust").unwrap();

        assert_eq!(
            ast.document
                .iter()
                .skip(1)
                .map(|l| l.borrow().kind)
                .collect::<Vec<Kind>>(),
            vec![Kind::CodeMark]
        );
        assert_eq!(
            ast._get_line(1).unwrap().borrow().buff,
            vec![
                Token::new("```".to_string(), TokenKind::CodeMark),
                Token::new("rust".to_string(), TokenKind::Text)
            ]
        );
    }

    #[test]
    fn test_code_block() {
        let md = "这是一个代码块的例子：
```
    let s = \"hello world\";
    let s1 = s.to_string();
```
    ```
    let s;
    assert_eq!(ast.doc[0].sequence[1].kind, TokenKind::Url);
    assert_eq!(ast.doc[0].sequence[1].value, url.to_string());";

        let mut ast = Ast::new();
        ast.parse_string(md).unwrap();

        assert_eq!(ast.count_lines(), md.lines().count());
        assert_eq!(ast.blocks.len(), 4);

        let kinds = vec![
            Kind::NormalText,
            Kind::Code,
            Kind::CodeMark,
            Kind::NormalText,
        ];
        assert_eq!(
            ast.blocks().iter().map(|b| b.kind()).collect::<Vec<Kind>>(),
            kinds
        );

        let counts: Vec<usize> = vec![1, 4, 1, 3];
        assert_eq!(
            ast.blocks()
                .iter()
                .map(|b| b._count())
                .collect::<Vec<usize>>(),
            counts
        );
    }
}
