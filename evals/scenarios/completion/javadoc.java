package app;

import java.io.IOException;
import java.util.List;

/**
 * A repository of items.
 *
 * @/*@class_tag*/
 */
public class Repo<T> {
    private int count;

    /**
     * Loads items. See {@link Li/*@link_type*/} and {@link #/*@link_member*/}.
     *
     * @param /*@param_name*/
     * @throws /*@throws_type*/
     * @see String#/*@see_member*/
     * @see java.util./*@see_qualified*/
     */
    public void load(String path, int limit) throws IOException {}

    /**
     * Removes everything.
     *
     * @/*@void_tag*/
     */
    public void clear() {}

    /**
     * Returns the size, see {@/*@inline_tag*/
     *
     * @param ignored not used
     * @param /*@param_rest*/
     */
    public int size(int ignored, int other) { return count; }

    /** Uses {@link ArrayLis/*@unimported*/}. */
    public List<T> items() { return null; }
}
//? complete class_tag has author param since see
//? complete class_tag lacks return throws
//? complete link_type has List
//? complete link_member has load clear size items
//? insert link_member load ~ "load(String, int)" plain
//? insert link_member size ~ "size(int, int)" plain
//? complete param_name order path limit
//? complete throws_type first IOException
//? complete see_member has length substring
//? insert see_member length ~ "length()" plain
//? complete see_qualified has ArrayList concurrent
//? complete void_tag has param throws since
//? complete void_tag lacks return
//? complete inline_tag has link linkplain code value inheritDoc literal
//? complete param_rest first other
//? insert unimported ArrayList ~ "ArrayList"
