import java.util.List;

class Main {
    int field;
    static final int LIMIT = 3;

    int helper(int v) { return v; }

    int run(int param, List<String> names) {
        int before = 1;
        {
            int innerOnly = 2;
        }
        for (String each : names) {
            each./*@in_loop*/;
        }
        /*@stmt*/
        int after = 3;
        return before;
    }
}
//? complete stmt has before param field LIMIT helper run names
//? complete stmt lacks after innerOnly each
//? complete stmt has int return if for
//? complete in_loop has length
