// Tuple `QueryData` impls (arity 1..=12), generated over `QueryElement`
// members. Included from `query/mod.rs`; not a standalone module.

macro_rules! impl_query_tuple {
    ($iter:ident, $iter_mut:ident, $n:expr, $($name:ident => $idx:tt),+) => {
        /// Shared zipped row iterator over a tuple query's archetype columns.
        #[allow(non_snake_case)]
        pub struct $iter<'a, $($name: QueryElement),+> {
            $($name: $name::Iter<'a>,)+
        }

        impl<'a, $($name: QueryElement),+> Iterator for $iter<'a, $($name),+> {
            type Item = ($($name::Item<'a>,)+);

            fn next(&mut self) -> Option<Self::Item> {
                Some(($(self.$name.next()?,)+))
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                let mut min = usize::MAX;
                $(min = min.min(self.$name.len());)+
                (min, Some(min))
            }
        }

        impl<'a, $($name: QueryElement),+> ExactSizeIterator for $iter<'a, $($name),+> {}

        /// Exclusive zipped row iterator over a tuple query's archetype columns.
        #[allow(non_snake_case)]
        pub struct $iter_mut<'a, $($name: QueryElement),+> {
            $($name: $name::IterMut<'a>,)+
        }

        impl<'a, $($name: QueryElement),+> Iterator for $iter_mut<'a, $($name),+> {
            type Item = ($($name::ItemMut<'a>,)+);

            fn next(&mut self) -> Option<Self::Item> {
                Some(($(self.$name.next()?,)+))
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                let mut min = usize::MAX;
                $(min = min.min(self.$name.len());)+
                (min, Some(min))
            }
        }

        impl<'a, $($name: QueryElement),+> ExactSizeIterator for $iter_mut<'a, $($name),+> {}

        impl<$($name: QueryElement),+> sealed::Sealed for ($($name,)+) {}

        #[allow(non_snake_case)]
        impl<$($name: QueryElement),+> QueryData for ($($name,)+) {
            type Item<'a> = ($($name::Item<'a>,)+);
            type ItemMut<'a> = ($($name::ItemMut<'a>,)+);
            type Iter<'a> = $iter<'a, $($name),+>;
            type IterMut<'a> = $iter_mut<'a, $($name),+>;

            fn access(visit: &mut dyn FnMut(TypeId, &'static str, bool)) {
                $($name::access(visit);)+
            }

            fn matches(archetype: &Archetype, registry: &ComponentRegistry) -> bool {
                $(match $name::required(registry) {
                    ElementNeed::Entity => {}
                    ElementNeed::Column(id) => {
                        if !archetype.contains(id) {
                            return false;
                        }
                    }
                    ElementNeed::Unregistered => return false,
                })+
                true
            }

            fn iter_archetype<'a>(
                archetype: &'a Archetype,
                registry: &ComponentRegistry,
            ) -> Option<Self::Iter<'a>> {
                let entities = archetype.entities();
                Some($iter {
                    $($name: {
                        let column = match $name::required(registry) {
                            ElementNeed::Entity => None,
                            ElementNeed::Column(id) => archetype.column(id),
                            ElementNeed::Unregistered => return None,
                        };
                        $name::make_iter(column, entities)?
                    },)+
                })
            }

            fn iter_archetype_mut<'a>(
                archetype: &'a mut Archetype,
                registry: &ComponentRegistry,
            ) -> Option<Self::IterMut<'a>> {
                let wanted: [Option<ComponentId>; $n] = [
                    $(match $name::required(registry) {
                        ElementNeed::Entity => None,
                        ElementNeed::Column(id) => Some(id),
                        ElementNeed::Unregistered => return None,
                    },)+
                ];
                let (entities, cols) = archetype.entities_and_columns_mut(wanted);
                let [$($name,)+] = cols;
                Some($iter_mut {
                    $($name: $name::make_iter_mut($name, entities)?,)+
                })
            }

            fn get_row<'a>(
                archetype: &'a Archetype,
                registry: &ComponentRegistry,
                row: usize,
            ) -> Option<Self::Item<'a>> {
                let mut iter = Self::iter_archetype(archetype, registry)?;
                $(let $name = iter.$name.nth(row)?;)+
                Some(($($name,)+))
            }

            fn get_row_mut<'a>(
                archetype: &'a mut Archetype,
                registry: &ComponentRegistry,
                row: usize,
            ) -> Option<Self::ItemMut<'a>> {
                let mut iter = Self::iter_archetype_mut(archetype, registry)?;
                $(let $name = iter.$name.nth(row)?;)+
                Some(($($name,)+))
            }
        }
    };
}

impl_query_tuple!(Iter1, IterMut1, 1, A => 0);
impl_query_tuple!(Iter2, IterMut2, 2, A => 0, B => 1);
impl_query_tuple!(Iter3, IterMut3, 3, A => 0, B => 1, C => 2);
impl_query_tuple!(Iter4, IterMut4, 4, A => 0, B => 1, C => 2, D => 3);
impl_query_tuple!(Iter5, IterMut5, 5, A => 0, B => 1, C => 2, D => 3, E => 4);
impl_query_tuple!(Iter6, IterMut6, 6, A => 0, B => 1, C => 2, D => 3, E => 4, F => 5);
impl_query_tuple!(Iter7, IterMut7, 7, A => 0, B => 1, C => 2, D => 3, E => 4, F => 5, G => 6);
impl_query_tuple!(Iter8, IterMut8, 8, A => 0, B => 1, C => 2, D => 3, E => 4, F => 5, G => 6, H => 7);
impl_query_tuple!(Iter9, IterMut9, 9, A => 0, B => 1, C => 2, D => 3, E => 4, F => 5, G => 6, H => 7, I => 8);
impl_query_tuple!(Iter10, IterMut10, 10, A => 0, B => 1, C => 2, D => 3, E => 4, F => 5, G => 6, H => 7, I => 8, J => 9);
impl_query_tuple!(Iter11, IterMut11, 11, A => 0, B => 1, C => 2, D => 3, E => 4, F => 5, G => 6, H => 7, I => 8, J => 9, K => 10);
impl_query_tuple!(Iter12, IterMut12, 12, A => 0, B => 1, C => 2, D => 3, E => 4, F => 5, G => 6, H => 7, I => 8, J => 9, K => 10, L => 11);
