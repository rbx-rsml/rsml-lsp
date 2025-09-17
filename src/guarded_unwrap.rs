#[macro_export]
macro_rules! guarded_unwrap {
    (@inner $expr:expr, $none_case:expr) => {
        match crate::guarded_unwrap::GuardedUnwrap::guarded_unwrap_inner($expr) {
            Some(value) => value,
            None => $none_case,
        }
    };

    ($expr:expr, return $label:lifetime $ret:expr) => {
        guarded_unwrap!(@inner $expr, { return $label $ret })
    };

    ($expr:expr, return $label:lifetime) => {
        guarded_unwrap!(@inner $expr, { break $label })
    };

    ($expr:expr, return $ret:expr) => {
        guarded_unwrap!(@inner $expr, { return $ret })
    };

    ($expr:expr, return) => {
        guarded_unwrap!(@inner $expr, { return })
    };

    ($expr:expr, break $label:lifetime $ret:expr) => {
        guarded_unwrap!(@inner $expr, { break $label $ret })
    };

    ($expr:expr, break $label:lifetime) => {
        guarded_unwrap!(@inner $expr, { break $label })
    };

    ($expr:expr, break $ret:expr) => {
        guarded_unwrap!(@inner $expr, { break $ret })
    };

    ($expr:expr, break) => {
        guarded_unwrap!(@inner $expr, { break })
    };

    ($expr:expr, continue $label:lifetime $ret:expr) => {
        guarded_unwrap!(@inner $expr, { continue $label $ret })
    };

    ($expr:expr, continue $label:lifetime) => {
        guarded_unwrap!(@inner $expr, { break $label })
    };

    ($expr:expr, continue $ret:expr) => {
        guarded_unwrap!(@inner $expr, { continue $ret })
    };

    ($expr:expr, continue) => {
        guarded_unwrap!(@inner $expr, { continue })
    };
}

pub trait GuardedUnwrap<'a, T> {
    fn guarded_unwrap_inner(self) -> Option<T>;
}

impl<'a, T> GuardedUnwrap<'a, T> for Option<T> {
    fn guarded_unwrap_inner(self) -> Option<T> {
        self
    }
}

impl<'a, T> GuardedUnwrap<'a, &'a T> for &'a Option<T> {
    fn guarded_unwrap_inner(self) -> Option<&'a T> {
        self.as_ref()
    }
}

impl<'a, T> GuardedUnwrap<'a, &'a mut T> for &'a mut Option<T> {
    fn guarded_unwrap_inner(self) -> Option<&'a mut T> {
        self.as_mut()
    }
}


impl<'a, T, E> GuardedUnwrap<'a, T> for Result<T, E> {
    fn guarded_unwrap_inner(self) -> Option<T> {
        self.ok()
    }
}

impl<'a, T, E> GuardedUnwrap<'a, &'a T> for &'a Result<T, E> {
    fn guarded_unwrap_inner(self) -> Option<&'a T> {
        self.as_ref().ok()
    }
}

impl<'a, T, E> GuardedUnwrap<'a, &'a mut T> for &'a mut Result<T, E> {
    fn guarded_unwrap_inner(self) -> Option<&'a mut T> {
        self.as_mut().ok()
    }
}

#[macro_export]
macro_rules! guarded_unwrap_advance {
    ($expr:expr, return Parsed (.., $parsed_expr:expr)) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => return Parsed (Some(node), $parsed_expr),
            None => return Parsed (None, $parsed_expr)
        }
    };

    ($expr:expr, continue Parsed (.., $parsed_expr:expr)) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => continue Parsed (Some(node), $parsed_expr),
            None => continue Parsed (None, $parsed_expr)
        }
    };

    ($expr:expr, break Parsed (.., $parsed_expr:expr)) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => break Parsed (Some(node), $parsed_expr),
            None => break Parsed (None, $parsed_expr)
        }
    };

    ($expr:expr, { $ret:expr; return Parsed (.., $parsed_expr:expr) }) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => { $ret; return Parsed (Some(node), $parsed_expr) },
            None => { $ret; return Parsed (None, $parsed_expr) }
        }
    };

    ($expr:expr, { $ret:expr; continue Parsed (.., $parsed_expr:expr) }) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => { $ret; continue Parsed (Some(node), $parsed_expr) },
            None => { $ret; continue Parsed (None, $parsed_expr) }
        }
    };

    ($expr:expr, { $ret:expr; break Parsed (.., $parsed_expr:expr) }) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => { $ret; break Parsed (Some(node), $parsed_expr) },
            None => { $ret; break Parsed (None, $parsed_expr) }
        }
    };

    ($expr:expr, return (NodeStatus::.., $parsed_expr:expr)) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => return (crate::parser::NodeStatus::Err(node), $parsed_expr),
            None => return (crate::parser::NodeStatus::None, $parsed_expr)
        }
    };

    ($expr:expr, continue (NodeStatus::.., $parsed_expr:expr)) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => continue (crate::parser::NodeStatus::Err(node), $parsed_expr),
            None => continue (crate::parser::NodeStatus::None, $parsed_expr)
        }
    };

    ($expr:expr, break (NodeStatus::.., $parsed_expr:expr)) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => break (crate::parser::NodeStatus::Err(node), $parsed_expr),
            None => break (crate::parser::NodeStatus::None, $parsed_expr)
        }
    };

    ($expr:expr, { $ret:expr; return (NodeStatus::.., $parsed_expr:expr) }) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => { $ret; return (crate::parser::NodeStatus::Err(node), $parsed_expr) },
            None => { $ret; return (crate::parser::NodeStatus::None, $parsed_expr) }
        }
    };

    ($expr:expr, { $ret:expr; continue (NodeStatus::.., $parsed_expr:expr) }) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => { $ret; continue (crate::parser::NodeStatus::Err(node), $parsed_expr) },
            None => { $ret; continue (crate::parser::NodeStatus::None, $parsed_expr) }
        }
    };

    ($expr:expr, { $ret:expr; break (NodeStatus::.., $parsed_expr:expr) }) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => { $ret; break (crate::parser::NodeStatus::Err(node), $parsed_expr) },
            None => { $ret; break (crate::parser::NodeStatus::None, $parsed_expr) }
        }
    };

    ($expr:expr, return $ret:expr) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(_)) => return $ret,
            None => return $ret
        }
    };

    ($expr:expr, continue $ret:expr) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(_)) => continue $ret,
            None => continue $ret
        }
    };

    ($expr:expr, break $ret:expr) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(_)) => break $ret,
            None => break $ret
        }
    };

    ($expr:expr, { $ret:expr; return }) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => { $ret; return Some(node) },
            None => { $ret; return None },
        }
    };

    ($expr:expr, { $ret:expr; continue }) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => { $ret; continue Some(node) },
            None => { $ret; continue None },
        }
    };

    ($expr:expr, { $ret:expr; break }) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => { $ret; break Some(node) },
            None => { $ret; break None },
        }
    };

    ($expr:expr, return) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => return Some(node),
            None => return None
        }
    };

    ($expr:expr, continue) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => continue Some(node),
            None => continue None
        }
    };

    ($expr:expr, break) => {
        match $expr {
            Some(Ok(node)) => node,
            Some(Err(node)) => break Some(node),
            None => break None
        }
    };
}