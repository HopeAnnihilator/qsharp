#[cfg(test)]
mod tests;

use std::{
    cell::RefCell, collections::HashMap, fmt::Display, iter::Peekable, ops::Deref, rc::Rc,
};


#[derive(Debug, Clone)]

pub struct NamespaceTreeRoot {
    assigner: usize,
    tree: NamespaceTreeNode,
}

/// An ID that corresponds to a namespace in the global scope.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Default)]
pub struct NamespaceId(usize);
impl NamespaceId {
    /// Create a new namespace ID.
    pub fn new(value: usize) -> Self {
        Self(value)
    }
}

impl From<usize> for NamespaceId {
    fn from(value: usize) -> Self {
        Self::new(value)
    }
}

impl From<NamespaceId> for usize {
    fn from(value: NamespaceId) -> Self {
        value.0
    }
}

impl From<&NamespaceId> for usize {
    fn from(value: &NamespaceId) -> Self {
        value.0
    }
}

impl Deref for NamespaceId {
    type Target = usize;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for NamespaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Namespace {}", self.0)
    }
}

impl NamespaceTreeRoot {
    /// Create a new namespace tree root. The assigner is used to assign new namespace IDs.
    pub fn new(assigner: usize, tree: NamespaceTreeNode) -> Self {
        Self { assigner, tree }
    }
    /// Get the namespace tree field. This is the root of the namespace tree.
    pub fn tree(&self) -> &NamespaceTreeNode {
        &self.tree
    }

    /// Insert a namespace into the tree. If the namespace already exists, return its ID.
    pub fn insert_or_find_namespace(
        &mut self,
        ns: impl IntoIterator<Item = Rc<str>>,
    ) -> NamespaceId {
        self.tree
            .insert_or_find_namespace(ns.into_iter().peekable(), &mut self.assigner)
            .expect("namespace creation should not fail")
    }

    pub fn new_namespace_node(
        &mut self,
        children: HashMap<Rc<str>, NamespaceTreeNode>,
    ) -> NamespaceTreeNode {
        self.assigner += 1;
        NamespaceTreeNode {
            id: NamespaceId::new(self.assigner),
            children,
        }
    }

    pub fn find_namespace(&self, ns: impl Into<Vec<Rc<str>>>) -> Option<NamespaceId> {
        self.tree.find_namespace(ns)
    }

    pub fn find_id(&self, id: &NamespaceId) -> (Vec<Rc<str>>, Rc<&NamespaceTreeNode>) {
        return self.tree.find_id(*id, vec![]);
    }

    pub fn root_id(&self) -> NamespaceId {
        self.tree.id
    }
}


impl Default for NamespaceTreeRoot {
    fn default() -> Self {
        Self {
            assigner: 0,
            tree: NamespaceTreeNode {
                children: HashMap::new(),
                id: NamespaceId::new(0),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct NamespaceTreeNode {
    pub children: HashMap<Rc<str>, NamespaceTreeNode>,
    pub id: NamespaceId,
}
impl NamespaceTreeNode {
    pub fn new(id: NamespaceId, children: HashMap<Rc<str>, NamespaceTreeNode>) -> Self {
        Self { children, id }
    }
    pub fn children(&self) -> &HashMap<Rc<str>, NamespaceTreeNode> {
        &self.children
    }
    fn get(&self, component: &Rc<str>) -> Option<&NamespaceTreeNode> {
        self.children.get(component)
    }
    pub fn id(&self) -> NamespaceId {
        self.id
    }
    fn contains(&self, ns: impl Into<Vec<Rc<str>>>) -> bool {
        self.find_namespace(ns).is_some()
    }
    fn find_namespace(&self, ns: impl Into<Vec<Rc<str>>>) -> Option<NamespaceId> {
        // look up a namespace in the tree and return the id
        // do a breadth-first search through NamespaceTree for the namespace
        // if it's not found, return None
        let mut buf = Rc::new(self);
        for component in ns.into().iter() {
            if let Some(next_ns) = buf.get(component) {
                buf = Rc::new(next_ns);
            } else {
                return None;
            }
        }
        return Some(buf.id);
    }

    /// If the namespace already exists, it will not be inserted.
    /// Returns the ID of the namespace.
    pub fn insert_or_find_namespace<I>(
        &mut self,
        mut iter: Peekable<I>,
        assigner: &mut usize,
    ) -> Option<NamespaceId>
    where
        I: Iterator<Item = Rc<str>>,
    {
        let next_item = match iter.next() {
            Some(item) => item,
            None => return None,
        };
        println!("Inserting namespace {}", next_item);

        let next_node = self.children.get_mut(&next_item);
        if let Some(mut next_node) = next_node {
            return next_node.insert_or_find_namespace(iter, assigner);
        } else {
            println!("creating new node");
            *assigner += 1;
            let mut new_node =
                NamespaceTreeNode::new(NamespaceId::new(*assigner), HashMap::new());
            if iter.peek().is_none() {
                let new_node_id = new_node.id;
                self.children.insert(next_item, new_node);
                return Some(new_node_id);
            } else {
                let id = new_node.insert_or_find_namespace(iter, assigner);
                self.children.insert(next_item, new_node);
                return id;
            }
        }
    }

    fn find_id(
        &self,
        id: NamespaceId,
        names_buf: Vec<Rc<str>>,
    ) -> (Vec<Rc<str>>, Rc<&NamespaceTreeNode>) {
        if self.id == id {
            return (names_buf, Rc::new(self));
        } else {
            for (name, node) in self.children.iter() {
                let mut new_buf = names_buf.clone();
                new_buf.push(name.clone());
                let (names, node) = node.find_id(id, new_buf);
                if names.len() > 0 {
                    return (names, node);
                }
            }
            return (vec![], Rc::new(self));
        }
    }
}