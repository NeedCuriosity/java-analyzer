class Main {
    /*@member*/

    int first(boolean flag, java.util.List<String> names) {
        ret/*@ret*/
        names.clear();
        return 0;
    }

    void postfix(java.util.List<String> names) {
        names.fo/*@member_prefix*/
    }

    void orElse(boolean flag) {
        if (flag) {
        } el/*@else_kw*/
    }

    void loop(java.util.List<String> names) {
        for (String name : names) {
            bre/*@in_loop*/
        }
        whi/*@while_kw*/
    }
}
//? complete member has public private static void class int
//? complete member lacks return
//? complete ret first return
//? complete member_prefix has forEach
//? complete else_kw has else
//? complete in_loop has break
//? complete while_kw has while
