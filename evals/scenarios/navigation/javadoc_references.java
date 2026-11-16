import java.util.List;

/**
 * Holds {@link /*@itemLink*/Item items}; see {@link #/*@addLink*/add(Item)} and
 * {@link Box#/*@sizeLink*/size}.
 *
 * @param </*@tParam*/T> the label type
 * @see /*@listLink*/List#/*@listGet*/get(int)
 */
class Box<T> {
    private int /*@sizeDecl*/size;

    /**
     * Adds an item.
     *
     * @param /*@itemParam*/item the item, never {@code null}
     * @throws /*@iaeLink*/IllegalArgumentException if {@link Item#/*@validLink*/valid()} fails
     */
    void /*@addDecl*/add(Item /*@itemDecl*/item) {
        if (!item.valid()) throw new IllegalArgumentException();
        size++;
    }

    /** Same as {@link #/*@addLink2*/add(Item)}. */
    void put(Item item) { /*@addCall*/add(item); }
}

class /*@itemCls*/Item {
    boolean /*@validDecl*/valid() { return true; }
}
//? def itemLink -> itemCls
//? def addLink -> addDecl
//? def sizeLink -> sizeDecl
//? def validLink -> validDecl
//? def itemParam -> itemDecl
//? hover listGet ~ "get(int"
//? hover iaeLink ~ "IllegalArgumentException"
//? refs addDecl = addLink addLink2 addCall
//? rename itemCls Element ; has "{@link Element items}" ; has "{@link Element#valid()} fails" ; has "add(Element)} and"
//? rename addDecl insert ; has "{@link #insert(Item)} and" ; has "Same as {@link #insert(Item)}" ; has "insert(item);"
//? rename itemDecl element ; has "@param element the item" ; has "void add(Item element)"
//? diag none
