use crate::ParseError;
use crate::subtree_parser::Parser;

#[derive(Clone)]
pub(crate) struct TtCursor<'a> {
    subtree: &'a tt::Subtree,
    pos: usize,
}

impl<'a> TtCursor<'a> {
    pub(crate) fn new(subtree: &'a tt::Subtree) -> TtCursor<'a> {
        TtCursor { subtree, pos: 0 }
    }

    pub(crate) fn is_eof(&self) -> bool {
        self.pos == self.subtree.token_trees.len()
    }

    pub(crate) fn current(&self) -> Option<&'a tt::TokenTree> {
        self.subtree.token_trees.get(self.pos)
    }

    pub(crate) fn at_punct(&self) -> Option<&'a tt::Punct> {
        match self.current() {
            Some(tt::TokenTree::Leaf(tt::Leaf::Punct(it))) => Some(it),
            _ => None,
        }
    }

    pub(crate) fn at_char(&self, char: char) -> bool {
        match self.at_punct() {
            Some(tt::Punct { char: c, .. }) if *c == char => true,
            _ => false,
        }
    }

    pub(crate) fn at_ident(&mut self) -> Option<&'a tt::Ident> {
        match self.current() {
            Some(tt::TokenTree::Leaf(tt::Leaf::Ident(i))) => Some(i),
            _ => None,
        }
    }

    pub(crate) fn at_literal(&mut self) -> Option<&'a tt::Literal> {
        match self.current() {
            Some(tt::TokenTree::Leaf(tt::Leaf::Literal(i))) => Some(i),
            _ => None,
        }
    }

    pub(crate) fn bump(&mut self) {
        self.pos += 1;
    }
    pub(crate) fn rev_bump(&mut self) {
        self.pos -= 1;
    }

    pub(crate) fn eat(&mut self) -> Option<&'a tt::TokenTree> {
        self.current().map(|it| {
            self.bump();
            it
        })
    }

    pub(crate) fn eat_subtree(&mut self) -> Result<&'a tt::Subtree, ParseError> {
        match self.current() {
            Some(tt::TokenTree::Subtree(sub)) => {
                self.bump();
                Ok(sub)
            }
            _ => Err(ParseError::Expected(String::from("subtree"))),
        }
    }

    pub(crate) fn eat_punct(&mut self) -> Option<&'a tt::Punct> {
        self.at_punct().map(|it| {
            self.bump();
            it
        })
    }

    pub(crate) fn eat_ident(&mut self) -> Option<&'a tt::Ident> {
        self.at_ident().map(|i| {
            self.bump();
            i
        })
    }

    pub(crate) fn eat_literal(&mut self) -> Option<&'a tt::Literal> {
        self.at_literal().map(|i| {
            self.bump();
            i
        })
    }

    pub(crate) fn eat_path(&mut self) -> Option<tt::TokenTree> {
        let parser = Parser::new(&mut self.pos, self.subtree);
        parser.parse_path()
    }

    pub(crate) fn eat_expr(&mut self) -> Option<tt::TokenTree> {
        let parser = Parser::new(&mut self.pos, self.subtree);
        parser.parse_expr()
    }

    pub(crate) fn eat_ty(&mut self) -> Option<tt::TokenTree> {
        let parser = Parser::new(&mut self.pos, self.subtree);
        parser.parse_ty()
    }

    pub(crate) fn eat_pat(&mut self) -> Option<tt::TokenTree> {
        let parser = Parser::new(&mut self.pos, self.subtree);
        parser.parse_pat()
    }

    pub(crate) fn eat_stmt(&mut self) -> Option<tt::TokenTree> {
        let parser = Parser::new(&mut self.pos, self.subtree);
        parser.parse_stmt()
    }

    pub(crate) fn eat_block(&mut self) -> Option<tt::TokenTree> {
        let parser = Parser::new(&mut self.pos, self.subtree);
        parser.parse_block()
    }

    pub(crate) fn eat_meta(&mut self) -> Option<tt::TokenTree> {
        let parser = Parser::new(&mut self.pos, self.subtree);
        parser.parse_meta()
    }

    pub(crate) fn eat_item(&mut self) -> Option<tt::TokenTree> {
        let parser = Parser::new(&mut self.pos, self.subtree);
        parser.parse_item()
    }

    pub(crate) fn eat_lifetime(&mut self) -> Option<tt::TokenTree> {
        // check if it start from "`"
        if let Some(ident) = self.at_ident() {
            if ident.text.chars().next()? != '\'' {
                return None;
            }
        }

        self.eat_ident().cloned().map(|ident| tt::Leaf::from(ident).into())
    }

    pub(crate) fn eat_vis(&mut self) -> Option<tt::TokenTree> {
        let parser = Parser::new(&mut self.pos, self.subtree);
        parser.parse_vis()
    }

    pub(crate) fn expect_char(&mut self, char: char) -> Result<(), ParseError> {
        if self.at_char(char) {
            self.bump();
            Ok(())
        } else {
            Err(ParseError::Expected(format!("`{}`", char)))
        }
    }
}
