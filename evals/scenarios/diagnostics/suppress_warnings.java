import java.util.List;

class Main {
    @SuppressWarnings("unused")
    private int quietField;

    private int /*@loud_field*/loudField;

    @SuppressWarnings({"unused", "deprecation"})
    private void quietMethod() {
        int local = 1;
        new java.util.Date(2020, 1, 1);
    }

    void locals(List<String> names) {
        @SuppressWarnings("unused") int quiet = 1;
        int /*@loud*/loud = 2;
        int unused = 3;
        int ignored = 4;
        //noinspection unused
        int quietByComment = 5;
        String s = null;
        //noinspection DataFlowIssue
        s.length();
        String t = null;
        /*@npe*/t.length();
    }

    @SuppressWarnings("all")
    void everything() {
        int a = 1;
        /*@error*/undefined();
    }

    void fixes(String x) {
        int /*@fix_local*/spare = 1;
        String n = null;
        /*@fix_stmt*/n.length();
    }

    @SuppressWarnings("unchecked")
    void existing() {
        int /*@fix_merge*/other = 1;
    }
}
//? diag unused-private-member loud_field
//? diag unused-local loud
//? diag unused-local fix_local
//? diag unused-local fix_merge
//? diag only unused-local
//? diag only unused-private-member
//? diag null-dereference npe
//? diag null-dereference fix_stmt
//? diag only null-dereference
//? diag unresolved-method error
//? diag only deprecated
//? assist suppress_declaration fix_local ; has "@SuppressWarnings(\"unused\")\n        int spare = 1;"
//? assist suppress_method fix_local ; has "@SuppressWarnings(\"unused\")\n    void fixes(String x) {"
//? assist suppress_class fix_local ; has "@SuppressWarnings(\"unused\")\nclass Main {"
//? assist suppress_statement fix_stmt ; has "        //noinspection DataFlowIssue\n        n.length();"
//? assist suppress_method fix_merge ; has "@SuppressWarnings({\"unchecked\", \"unused\"})\n    void existing() {"
