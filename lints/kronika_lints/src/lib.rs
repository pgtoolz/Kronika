#![feature(rustc_private)]

extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_session;

use clippy_utils::diagnostics::span_lint;
use rustc_hir::def::Res;
use rustc_hir::{
    Arm, BorrowKind, CaptureBy, ClosureKind, Expr, ExprKind, MatchSource, Mutability, PatExpr,
    PatExprKind, PatKind, QPath,
};
use rustc_lint::{LateContext, LateLintPass, LintStore};
use rustc_middle::ty;
use rustc_session::{Session, declare_lint, impl_lint_pass};

dylint_linting::dylint_library!();

declare_lint! {
    /// Detects matches that map every fieldless enum variant to itself.
    pub IDENTITY_ENUM_MATCH,
    Warn,
    "fieldless enum match returns the unchanged variant"
}

declare_lint! {
    /// Detects a borrowed zero-argument closure used to adapt an already borrowed callable.
    pub BORROWED_FORWARDING_CLOSURE,
    Warn,
    "borrowed zero-argument closure adapts an already borrowed callable"
}

struct RepoLints;

impl_lint_pass!(RepoLints => [IDENTITY_ENUM_MATCH, BORROWED_FORWARDING_CLOSURE]);

impl<'tcx> LateLintPass<'tcx> for RepoLints {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        check_identity_enum_match(cx, expr);
        check_borrowed_forwarding_closure(cx, expr);
    }
}

#[unsafe(no_mangle)]
pub fn register_lints(session: &Session, lint_store: &mut LintStore) {
    dylint_linting::init_config(session);
    lint_store.register_lints(&[IDENTITY_ENUM_MATCH, BORROWED_FORWARDING_CLOSURE]);
    lint_store.register_late_pass(|_| Box::new(RepoLints));
}

fn check_identity_enum_match(cx: &LateContext<'_>, expr: &Expr<'_>) {
    let ExprKind::Match(scrutinee, arms, source) = expr.kind else {
        return;
    };
    if !matches!(source, MatchSource::Normal | MatchSource::Postfix) {
        return;
    }
    let scrutinee_ty = cx.typeck_results().expr_ty(scrutinee);
    let ty::Adt(definition, _) = scrutinee_ty.kind() else {
        return;
    };
    if !definition.is_enum()
        || definition
            .variants()
            .iter()
            .any(|variant| !variant.fields.is_empty())
        || arms.is_empty()
        || scrutinee_ty != cx.typeck_results().expr_ty(expr)
        || !arms.iter().all(|arm| identity_arm(cx, arm))
    {
        return;
    }
    span_lint(
        cx,
        IDENTITY_ENUM_MATCH,
        expr.span,
        "this match returns its fieldless enum input unchanged",
    );
}

fn identity_arm(cx: &LateContext<'_>, arm: &Arm<'_>) -> bool {
    if arm.guard.is_some() {
        return false;
    }
    let PatKind::Expr(pattern) = arm.pat.kind else {
        return false;
    };
    same_constructor(cx, pattern, arm.body)
}

fn same_constructor(cx: &LateContext<'_>, pattern: &PatExpr<'_>, body: &Expr<'_>) -> bool {
    let PatExprKind::Path(pattern_path) = pattern.kind else {
        return false;
    };
    let ExprKind::Path(body_path) = body.kind else {
        return false;
    };
    cx.typeck_results().qpath_res(&pattern_path, pattern.hir_id)
        == cx.typeck_results().qpath_res(&body_path, body.hir_id)
}

fn check_borrowed_forwarding_closure(cx: &LateContext<'_>, expr: &Expr<'_>) {
    let ExprKind::AddrOf(BorrowKind::Ref, Mutability::Not, closure_expr) = expr.kind else {
        return;
    };
    let ExprKind::Closure(closure) = closure_expr.kind else {
        return;
    };
    if closure.capture_clause != CaptureBy::Ref
        || !matches!(closure.kind, ClosureKind::Closure)
        || !closure.fn_decl.inputs.is_empty()
        || expr.span.from_expansion()
    {
        return;
    }

    let body = cx.tcx.hir_body(closure.body);
    if !body.params.is_empty() || body.value.span.from_expansion() {
        return;
    }
    let ExprKind::Call(callee, []) = body.value.kind else {
        return;
    };
    let ExprKind::Path(QPath::Resolved(None, path)) = callee.kind else {
        return;
    };
    if !matches!(path.res, Res::Local(_))
        || !matches!(
            cx.typeck_results().expr_ty(callee).kind(),
            ty::Ref(_, _, Mutability::Not)
        )
    {
        return;
    }

    span_lint(
        cx,
        BORROWED_FORWARDING_CLOSURE,
        expr.span,
        "this borrowed closure only forwards to an already borrowed zero-argument callable; widen the receiving callable bound with `?Sized` when appropriate",
    );
}
