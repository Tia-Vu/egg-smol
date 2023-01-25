use crate::*;

pub(crate) type Fresh = dyn FnMut() -> Symbol;

fn desugar_datatype(name: Symbol, variants: Vec<Variant>) -> Vec<Command> {
    vec![Command::Sort(name, None)]
        .into_iter()
        .chain(variants.into_iter().map(|variant| {
            Command::Function(FunctionDecl {
                name: variant.name,
                schema: Schema {
                    input: variant.types,
                    output: name,
                },
                merge: None,
                merge_action: vec![],
                default: None,
                cost: variant.cost,
            })
        }))
        .collect()
}

fn desugar_rewrite(ruleset: Symbol, rewrite: &Rewrite) -> Vec<Command> {
    let var = Symbol::from("rewrite_var__");
    vec![Command::FlatRule(
        ruleset,
        flatten_rule(Rule {
            body: [Fact::Eq(vec![Expr::Var(var), rewrite.lhs.clone()])]
                .into_iter()
                .chain(rewrite.conditions.clone())
                .collect(),
            head: vec![Action::Union(Expr::Var(var), rewrite.rhs.clone())],
        }),
    )]
}

fn desugar_birewrite(ruleset: Symbol, rewrite: &Rewrite) -> Vec<Command> {
    let rw2 = Rewrite {
        lhs: rewrite.rhs.clone(),
        rhs: rewrite.lhs.clone(),
        conditions: rewrite.conditions.clone(),
    };
    desugar_rewrite(ruleset, rewrite)
        .into_iter()
        .chain(desugar_rewrite(ruleset, &rw2))
        .collect()
}

// TODO use an egraph to perform the SSA translation without introducing
// so many fresh variables
fn expr_to_ssa(
    expr: &Expr,
    get_fresh: &mut Fresh,
    varUsed: &mut HashSet<Symbol>,
    varJustUsed: &mut HashSet<Symbol>,
    res: &mut Vec<SSAFact>,
    constraints: &mut Vec<SSAFact>,
) -> Symbol {
    match expr {
        Expr::Lit(l) => {
            let fresh = get_fresh();
            res.push(SSAFact::Assign(fresh, SSAExpr::Lit(l.clone())));
            fresh
        }
        Expr::Var(v) => {
            if varUsed.insert(*v) {
                varJustUsed.insert(*v);
                *v
            } else {
                let fresh = get_fresh();
                // logic to satisfy typechecker
                // if we used the variable in this recurrence, add the constraint afterwards
                if varJustUsed.contains(v) {
                    constraints.push(SSAFact::ConstrainEq(fresh, *v));
                // otherwise add the constrain immediately so we have the type
                } else {
                    res.push(SSAFact::ConstrainEq(fresh, *v));
                }
                fresh
            }
        }
        Expr::Call(f, children) => {
            let mut new_children = vec![];
            for child in children {
                new_children.push(expr_to_ssa(child, get_fresh, varUsed, varJustUsed, res, constraints));
            }
            let fresh = get_fresh();
            res.push(SSAFact::Assign(
                fresh,
                SSAExpr::Call(f.clone(), new_children),
            ));
            fresh
        }
    }
}

fn flatten_equalities(equalities: Vec<(Symbol, Expr)>, get_fresh: &mut Fresh) -> Vec<SSAFact> {
    let mut res = vec![];
    
    let mut varUsed = Default::default();
    for (lhs, rhs) in equalities {
        let mut constraints = vec![];
        let result = expr_to_ssa(&rhs, get_fresh, &mut varUsed, &mut Default::default(),  &mut res, &mut constraints);
        res.extend(constraints);

        if varUsed.insert(lhs) {
            res.push(SSAFact::ConstrainEq(lhs, result));
        }
    }
    res
}

fn flatten_facts(facts: &Vec<Fact>, get_fresh: &mut Fresh) -> Vec<SSAFact> {
    let mut equalities = vec![];
    for fact in facts {
        match fact {
            Fact::Eq(args) => {
                assert!(args.len() == 2);
                let lhs = &args[0];
                let rhs = &args[1];
                if let Expr::Var(v) = lhs {
                    equalities.push((v.clone(), rhs.clone()));
                } else if let Expr::Var(v) = rhs {
                    equalities.push((v.clone(), lhs.clone()));  
                } else {
                    let fresh = get_fresh();
                    equalities.push((fresh, lhs.clone()));
                    equalities.push((fresh, rhs.clone()));
                }
            }
            Fact::Fact(expr) => {
                equalities.push((get_fresh(), expr.clone()));
            }
        }
    }

    flatten_equalities(equalities, get_fresh)
}

fn expr_to_flat_actions(
    assign: Symbol,
    expr: &Expr,
    get_fresh: &mut Fresh,
    res: &mut Vec<SSAAction>,
) {
    match expr {
        Expr::Lit(l) => {
            res.push(SSAAction::Let(assign, SSAExpr::Lit(l.clone())));
        }
        Expr::Var(v) => {
            res.push(SSAAction::LetVar(assign, v.clone()));
        }
        Expr::Call(f, children) => {
            let mut new_children = vec![];
            for child in children {
                let fresh = get_fresh();
                expr_to_flat_actions(fresh, child, get_fresh, res);
                new_children.push(fresh);
            }
            res.push(SSAAction::Let(
                assign,
                SSAExpr::Call(f.clone(), new_children),
            ));
        }
    }
}

fn flatten_actions(actions: &Vec<Action>, get_fresh: &mut Fresh) -> Vec<SSAAction> {
    let mut add_expr = |expr: Expr, res: &mut Vec<SSAAction>| {
        let fresh = get_fresh();
        expr_to_flat_actions(fresh, &expr, get_fresh, res);
        fresh
    };

    let mut res = vec![];

    for action in actions {
        match action {
            Action::Let(symbol, expr) => {
                let added = add_expr(expr.clone(), &mut res);
                res.push(SSAAction::LetVar(*symbol, added));
            }
            Action::Set(symbol, exprs, rhs) => {
                let set = SSAAction::Set(
                    *symbol,
                    exprs
                        .clone()
                        .into_iter()
                        .map(|ex| add_expr(ex, &mut res))
                        .collect(),
                    add_expr(rhs.clone(), &mut res),
                );
                res.push(set);
            }
            Action::Delete(symbol, exprs) => {
                let del = SSAAction::Delete(
                    *symbol,
                    exprs
                        .clone()
                        .into_iter()
                        .map(|ex| add_expr(ex, &mut res))
                        .collect(),
                );
                res.push(del);
            }
            Action::Union(lhs, rhs) => {
                let un = SSAAction::Union(
                    add_expr(lhs.clone(), &mut res),
                    add_expr(rhs.clone(), &mut res),
                );
                res.push(un);
            }
            Action::Panic(msg) => {
                res.push(SSAAction::Panic(msg.clone()));
            }
            Action::Expr(expr) => {
                add_expr(expr.clone(), &mut res);
            }
        };
    }

    res
}

fn flatten_rule(rule: Rule) -> FlatRule {
    let mut varcount = 0;
    let mut get_fresh = move || {
        varcount += 1;
        Symbol::from(format!("fvar{}__", varcount))
    };

    FlatRule {
        head: flatten_actions(&rule.head, &mut get_fresh),
        body: flatten_facts(&rule.body, &mut get_fresh),
    }
}

pub(crate) fn desugar_command(egraph: &EGraph, command: Command) -> Result<Vec<Command>, Error> {
    Ok(match command {
        Command::Datatype { name, variants } => desugar_datatype(name, variants),
        Command::Rewrite(ruleset, rewrite) => desugar_rewrite(ruleset, &rewrite),
        Command::BiRewrite(ruleset, rewrite) => desugar_birewrite(ruleset, &rewrite),
        Command::Include(file) => {
            let s = std::fs::read_to_string(&file)
                .unwrap_or_else(|_| panic!("Failed to read file {file}"));
            egraph.parse_program(&s)?
        }
        Command::Rule(ruleset, rule) => vec![Command::FlatRule(ruleset, flatten_rule(rule))],
        _ => vec![command],
    })
}

pub(crate) fn desugar_program(
    egraph: &EGraph,
    program: Vec<Command>,
) -> Result<Vec<Command>, Error> {
    let intermediate: Result<Vec<Vec<Command>>, Error> = program
        .into_iter()
        .map(|command| desugar_command(egraph, command))
        .collect();
    intermediate.map(|v| v.into_iter().flatten().collect())
}

pub fn to_rules(program: Vec<Command>) -> Vec<Command> {
    program
        .into_iter()
        .map(|command| match command {
            Command::FlatRule(ruleset, rule) => Command::Rule(ruleset, rule.to_rule()),
            _ => command,
        })
        .collect()
}
